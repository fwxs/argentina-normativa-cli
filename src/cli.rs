//! Command-line arguments (clap).

use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, value_parser};

use crate::model::Jurisdiccion;
use crate::site::{DEFAULT_MAX_PAGES, LAW_TYPES};

/// Scraper for provincial laws published on argentina.gob.ar/normativa.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the provinces of the "Elegí una provincia" select box as a JSON array.
    List {
        #[command(subcommand)]
        scope: Option<ListScope>,
    },
    /// Search the laws of one province (or, with `national`, national norms) as JSON lines on stdout.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Query {
        /// Province name exactly as printed by `list`.
        #[arg(long, required = true)]
        province: Option<String>,
        /// Keywords for "Buscá por palabras clave", e.g. "impuesto tasa".
        #[arg(long, required = true)]
        query: Option<String>,
        #[command(subcommand)]
        scope: Option<QueryScope>,
    },
    /// Print a law's province, title and status as JSON and save its text as a PDF (default `<law>.pdf`).
    Fetch {
        /// Jurisdiction segment of the law url (only `provincial` is supported for now).
        #[arg(long)]
        jurisdiction: Jurisdiccion,
        /// Law slug, last segment of the law url, e.g. "ley-11035-123456789-0abc-defg-373-0000svorpyel".
        #[arg(long)]
        law: String,
        /// File the PDF is written to; defaults to `<law>.pdf` in the current directory.
        #[arg(long, value_name = "FILE_PATH")]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ListScope {
    /// Print the values the national search form accepts, as a JSON array.
    National {
        #[command(subcommand)]
        what: NationalList,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Subcommand)]
pub enum NationalList {
    /// "Organismo o dependencia" names, for `query national --agency`.
    Agencies,
    /// "Tipo de norma" slugs, for `query national --law-type`.
    LawType,
    /// "Año" values (newest first), for `query national --year`.
    Years,
}

#[derive(Debug, Subcommand)]
pub enum QueryScope {
    /// Search national norms with the filters of the site's national form.
    National(NationalArgs),
}

/// Arguments of `query national`: the form filters plus a crawl cap.
#[derive(Debug, Args, PartialEq)]
pub struct NationalArgs {
    #[command(flatten)]
    pub(crate) filters: NationalFilters,
    /// Stop after this many result pages (50 rows each, one crawl delay per page).
    #[arg(long, default_value_t = DEFAULT_MAX_PAGES, value_parser = value_parser!(u32).range(1..))]
    pub(crate) max_pages: u32,
}

/// clap parser for free-text filters: trims, and rejects empty values so they can't pass for a filter.
pub(crate) fn non_empty_trimmed(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("value must not be empty".to_owned());
    }
    Ok(trimmed.to_owned())
}

/// Fields of the national search form; every one is optional but at least one must be given.
#[derive(Debug, Default, Args, PartialEq)]
pub struct NationalFilters {
    /// "Tipo de norma": site slug, e.g. leyes, decretos, resoluciones.
    #[arg(long, value_parser = PossibleValuesParser::new(LAW_TYPES.iter().copied()))]
    pub(crate) law_type: Option<String>,
    /// "Número": norm number, digits only.
    #[arg(long, value_parser = value_parser!(u64))]
    pub(crate) law_number: Option<u64>,
    /// "Año": four-digit year (the site's year select starts at 1853).
    #[arg(long, value_parser = value_parser!(u16).range(1853..))]
    pub(crate) year: Option<u16>,
    /// "Organismo o dependencia": exact (upper-case) agency name, see `list national agencies`.
    #[arg(long, value_parser = non_empty_trimmed)]
    pub(crate) agency: Option<String>,
    /// "Publicación desde": YYYY-MM-DD.
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = non_empty_trimmed)]
    pub(crate) from_date: Option<String>,
    /// "Publicación hasta": YYYY-MM-DD.
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = non_empty_trimmed)]
    pub(crate) to_date: Option<String>,
    /// Keywords for "Buscá por palabras clave", e.g. "impuesto".
    #[arg(long, value_parser = non_empty_trimmed)]
    pub(crate) query: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn cli_list_without_arguments_parses() {
        // Act
        let cli = Cli::try_parse_from(["argentina-normativa-cli", "list"]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli {
                command: Command::List { scope: None }
            })
        ));
    }

    #[test]
    fn cli_list_national_subcommands_parse() {
        // Arrange
        let cases = [
            ("agencies", NationalList::Agencies),
            ("law-type", NationalList::LawType),
            ("years", NationalList::Years),
        ];

        for (name, expected) in cases {
            // Act
            let cli = Cli::try_parse_from(["argentina-normativa-cli", "list", "national", name]);

            // Assert
            assert!(
                matches!(
                    cli,
                    Ok(Cli {
                        command: Command::List {
                            scope: Some(ListScope::National { what })
                        }
                    }) if what == expected
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn cli_list_national_without_or_with_unknown_list_fails() {
        // Act
        let missing = Cli::try_parse_from(["argentina-normativa-cli", "list", "national"]);
        let unknown = Cli::try_parse_from(["argentina-normativa-cli", "list", "national", "bogus"]);

        // Assert
        assert!(missing.is_err());
        assert!(unknown.is_err());
    }

    #[test]
    fn cli_query_with_flags_parses_province_and_query() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "--province",
            "Córdoba",
            "--query",
            "impuesto tasa",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { province: Some(province), query: Some(query), scope: None } })
                if province == "Córdoba" && query == "impuesto tasa"
        ));
    }

    #[test]
    fn cli_query_without_province_fails() {
        // Act
        let cli = Cli::try_parse_from(["argentina-normativa-cli", "query", "--query", "impuesto"]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_national_with_flags_parses_every_filter() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-type",
            "resoluciones",
            "--law-number",
            "5911",
            "--year",
            "2026",
            "--agency",
            "AGENCIA DE RECAUDACION Y CONTROL ADUANERO",
            "--from-date",
            "2026-01-01",
            "--to-date",
            "2026-12-31",
            "--query",
            "impuesto",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { province: None, query: None, scope: Some(QueryScope::National(args)) } })
                if args.max_pages == DEFAULT_MAX_PAGES && args.filters == NationalFilters {
                    law_type: Some("resoluciones".to_owned()),
                    law_number: Some(5911),
                    year: Some(2026),
                    agency: Some("AGENCIA DE RECAUDACION Y CONTROL ADUANERO".to_owned()),
                    from_date: Some("2026-01-01".to_owned()),
                    to_date: Some("2026-12-31".to_owned()),
                    query: Some("impuesto".to_owned()),
                }
        ));
    }

    #[test]
    fn cli_query_national_empty_or_blank_values_fail() {
        // Act / Assert
        for flag in ["--query", "--agency", "--from-date", "--to-date"] {
            for blank in ["", "   "] {
                let cli = Cli::try_parse_from([
                    "argentina-normativa-cli",
                    "query",
                    "national",
                    flag,
                    blank,
                ]);
                assert!(cli.is_err(), "accepted `{flag} {blank:?}`");
            }
        }
    }

    #[test]
    fn cli_query_national_padded_values_are_trimmed() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "  impuesto ",
            "--agency",
            " MINISTERIO DE ECONOMIA",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { scope: Some(QueryScope::National(args)), .. } })
                if args.filters.query.as_deref() == Some("impuesto")
                    && args.filters.agency.as_deref() == Some("MINISTERIO DE ECONOMIA")
        ));
    }

    #[test]
    fn cli_query_national_max_pages_flag_overrides_default() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "impuesto",
            "--max-pages",
            "3",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { scope: Some(QueryScope::National(args)), .. } })
                if args.max_pages == 3
        ));
    }

    #[test]
    fn cli_query_national_zero_max_pages_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "impuesto",
            "--max-pages",
            "0",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_national_unknown_law_type_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-type",
            "ordenanzas",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_national_non_numeric_law_number_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-number",
            "12a",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_province_flags_with_national_subcommand_fail() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "--province",
            "Córdoba",
            "national",
            "--query",
            "impuesto",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_with_flags_parses_jurisdiction_and_law() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
            "--law",
            LAW_SLUG,
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Fetch { jurisdiction: Jurisdiccion::Provincial, law, output: None } })
                if law == LAW_SLUG
        ));
    }

    #[test]
    fn cli_fetch_unknown_jurisdiction_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "municipal",
            "--law",
            "ley-1",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_without_law_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_with_output_flag_parses_file_path() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
            "--law",
            "ley-1",
            "--output",
            "laws/ley-1.pdf",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Fetch { output: Some(output), .. } })
                if output == Path::new("laws/ley-1.pdf")
        ));
    }
}
