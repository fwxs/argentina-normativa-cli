//! Input checks that run before any request is sent.

use std::path::Path;

use anyhow::{Result, bail};

use crate::cli::{NationalFilters, ProvinceArgs};
use crate::model::Jurisdiccion;
use crate::site::is_law_slug;

/// `--law` ends up in a url path and a file name, so only slug characters are allowed.
pub(crate) fn validate_law_slug(ley: &str) -> Result<()> {
    if !is_law_slug(ley) {
        bail!("invalid law `{ley}`: expected a slug of letters, digits and dashes");
    }
    Ok(())
}

/// National norms are addressed as `norma-<id>`, e.g. `norma-431079`.
fn validate_national_slug(ley: &str) -> Result<()> {
    let has_id = ley
        .strip_prefix("norma-")
        .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()));
    if !has_id {
        bail!("invalid law `{ley}`: a national norm is `norma-` followed by its number");
    }
    Ok(())
}

pub(crate) fn validate_fetch(
    jurisdiccion: Jurisdiccion,
    ley: &str,
    output: Option<&Path>,
) -> Result<()> {
    validate_law_slug(ley)?;
    if jurisdiccion == Jurisdiccion::Nacional {
        validate_national_slug(ley)?;
    }
    output.map_or(Ok(()), validate_output_path)
}

/// `--output` is written only after two requests and a crawl delay, so check it can be written
/// to first: it must not be a directory and its parent directory must exist.
fn validate_output_path(output: &Path) -> Result<()> {
    if output.is_dir() {
        bail!("invalid output `{}`: it is a directory", output.display());
    }
    // A bare file name has an empty parent, which means the current directory.
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        bail!(
            "invalid output `{}`: `{}` is not an existing directory",
            output.display(),
            parent.display()
        );
    }
    Ok(())
}

/// `month` is 1..=12.
pub(crate) fn days_in_month(year: u32, month: u32) -> u32 {
    let is_leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    match month {
        2 if is_leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The site only accepts ISO dates in the url (`dd-mm-aaaa`, the form's placeholder, finds nothing).
pub(crate) fn validate_iso_date(flag: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = value.split('-').collect();
    let is_valid = value.chars().all(|c| c.is_ascii_digit() || c == '-')
        && match parts.as_slice() {
            [year, month, day] if year.len() == 4 && month.len() == 2 && day.len() == 2 => {
                match (
                    year.parse::<u32>(),
                    month.parse::<u32>(),
                    day.parse::<u32>(),
                ) {
                    (Ok(year), Ok(month @ 1..=12), Ok(day)) => {
                        (1..=days_in_month(year, month)).contains(&day)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
    if !is_valid {
        bail!("invalid {flag} `{value}`: expected YYYY-MM-DD");
    }
    Ok(())
}

/// Each given date must be a real ISO date, and the range must not be reversed.
pub(crate) fn validate_date_range(from_date: Option<&str>, to_date: Option<&str>) -> Result<()> {
    if let Some(from_date) = from_date {
        validate_iso_date("--from-date", from_date)?;
    }
    if let Some(to_date) = to_date {
        validate_iso_date("--to-date", to_date)?;
    }
    if let (Some(from_date), Some(to_date)) = (from_date, to_date)
        && from_date > to_date
    {
        bail!("--from-date {from_date} is after --to-date {to_date}");
    }
    Ok(())
}

pub(crate) fn validate_provinces(args: &ProvinceArgs) -> Result<()> {
    if args.query.is_none()
        && args.year.is_none()
        && args.law_number.is_none()
        && args.from_date.is_none()
        && args.to_date.is_none()
    {
        // An unfiltered search is every law of the province (thousands, one crawl delay per page).
        bail!("pass at least one filter, e.g. --query or --year");
    }
    validate_date_range(args.from_date.as_deref(), args.to_date.as_deref())
}

pub(crate) fn validate_national(filters: &NationalFilters) -> Result<()> {
    if *filters == NationalFilters::default() {
        // An unfiltered search is every norm (~200 pages at one page per crawl delay).
        bail!("pass at least one filter, e.g. --query or --law-type");
    }
    // The site answers this combination with a page that has no results block at all.
    if filters.law_type.as_deref() == Some("leyes") && filters.year.is_some() {
        bail!("--year finds no `leyes` on the site; use --from-date and --to-date instead");
    }
    validate_date_range(filters.from_date.as_deref(), filters.to_date.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    fn province_args() -> ProvinceArgs {
        ProvinceArgs {
            province: "Buenos Aires".to_owned(),
            law_number: None,
            year: None,
            from_date: None,
            to_date: None,
            query: None,
        }
    }

    #[test]
    fn validate_provinces_no_filters_is_rejected() {
        // Act
        let result = validate_provinces(&province_args());

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("at least one filter")));
    }

    #[test]
    fn validate_provinces_any_single_filter_is_accepted() {
        // Arrange
        let single_filters = [
            ProvinceArgs {
                query: Some("impuesto".to_owned()),
                ..province_args()
            },
            ProvinceArgs {
                year: Some(2020),
                ..province_args()
            },
            ProvinceArgs {
                law_number: Some(14709),
                ..province_args()
            },
            ProvinceArgs {
                from_date: Some("2020-01-01".to_owned()),
                ..province_args()
            },
            ProvinceArgs {
                to_date: Some("2020-12-31".to_owned()),
                ..province_args()
            },
        ];

        // Act / Assert
        for args in &single_filters {
            assert!(validate_provinces(args).is_ok(), "rejected {args:?}");
        }
    }

    #[test]
    fn validate_provinces_bad_or_impossible_dates_are_rejected() {
        // Act / Assert
        for invalid in ["01-01-2020", "2020-13-01", "2023-02-29", "hoy"] {
            let args = ProvinceArgs {
                from_date: Some(invalid.to_owned()),
                ..province_args()
            };
            assert!(validate_provinces(&args).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_provinces_reversed_date_range_is_rejected() {
        // Arrange
        let args = ProvinceArgs {
            from_date: Some("2020-02-01".to_owned()),
            to_date: Some("2020-01-01".to_owned()),
            ..province_args()
        };

        // Act
        let result = validate_provinces(&args);

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("after --to-date")));
    }

    #[test]
    fn validate_national_no_filters_is_rejected() {
        // Act
        let result = validate_national(&NationalFilters::default());

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("at least one filter")));
    }

    #[test]
    fn validate_national_leyes_with_year_is_rejected() {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("leyes".to_owned()),
            year: Some(2024),
            ..Default::default()
        };

        // Act / Assert
        assert!(validate_national(&filters).is_err());
    }

    #[test]
    fn validate_national_bad_dates_are_rejected() {
        // Act / Assert
        for invalid in [
            "01-01-2024",
            "2024-13-01",
            "2024-1-1",
            "2024-01-+1",
            "",
            "hoy",
        ] {
            let filters = NationalFilters {
                from_date: Some(invalid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_national_impossible_calendar_dates_are_rejected() {
        // Act / Assert
        for invalid in [
            "2024-02-30",
            "2023-02-29",
            "2024-04-31",
            "1900-02-29",
            "2024-00-10",
            "2024-06-00",
        ] {
            let filters = NationalFilters {
                to_date: Some(invalid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_national_leap_days_are_accepted() {
        // Act / Assert
        for valid in ["2024-02-29", "2000-02-29", "2023-02-28", "2024-12-31"] {
            let filters = NationalFilters {
                to_date: Some(valid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_ok(), "rejected `{valid}`");
        }
    }

    #[test]
    fn validate_national_reversed_date_range_is_rejected() {
        // Arrange
        let filters = NationalFilters {
            from_date: Some("2024-02-01".to_owned()),
            to_date: Some("2024-01-01".to_owned()),
            ..Default::default()
        };

        // Act
        let result = validate_national(&filters);

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("after --to-date")));
    }

    #[test]
    fn validate_national_valid_filters_are_accepted() {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("decretos".to_owned()),
            year: Some(2024),
            from_date: Some("2024-01-01".to_owned()),
            to_date: Some("2024-01-31".to_owned()),
            ..Default::default()
        };

        // Act / Assert
        assert!(validate_national(&filters).is_ok());
    }

    #[test]
    fn validate_law_slug_real_slug_is_accepted() {
        // Act / Assert
        assert!(validate_law_slug(LAW_SLUG).is_ok());
    }

    #[test]
    fn validate_law_slug_path_like_or_empty_values_are_rejected() {
        // Act / Assert
        for invalid in ["", "../x", "a/b", "ley 1", "ley-1.pdf", "ley\u{301}"] {
            assert!(validate_law_slug(invalid).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_fetch_nacional_norma_slug_is_accepted() {
        // Act / Assert
        assert!(validate_fetch(Jurisdiccion::Nacional, "norma-431079", None).is_ok());
    }

    #[test]
    fn validate_fetch_nacional_other_slugs_are_rejected() {
        // Act / Assert
        for invalid in ["ley-1", "norma-", "norma-12a", "norma431079", "../x", ""] {
            assert!(
                validate_fetch(Jurisdiccion::Nacional, invalid, None).is_err(),
                "accepted `{invalid}`"
            );
        }
    }

    const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

    #[test]
    fn validate_fetch_without_output_is_accepted() {
        // Act / Assert
        assert!(validate_fetch(Jurisdiccion::Provincial, "ley-1", None).is_ok());
    }

    #[test]
    fn validate_fetch_bare_file_name_is_accepted() {
        // Act / Assert
        let output = Path::new("ley-1.md");
        assert!(validate_fetch(Jurisdiccion::Provincial, "ley-1", Some(output)).is_ok());
    }

    #[test]
    fn validate_fetch_file_in_existing_directory_is_accepted() {
        // Arrange
        let output = Path::new(MANIFEST_DIR).join("tests/fixtures/out.md");

        // Act / Assert
        assert!(validate_fetch(Jurisdiccion::Provincial, "ley-1", Some(&output)).is_ok());
    }

    #[test]
    fn validate_fetch_missing_parent_directory_returns_error() {
        // Arrange
        let output = Path::new(MANIFEST_DIR).join("no-such-dir/out.md");

        // Act
        let result = validate_fetch(Jurisdiccion::Provincial, "ley-1", Some(&output));

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("not an existing directory")));
    }

    #[test]
    fn validate_fetch_parent_is_a_file_returns_error() {
        // Arrange
        let output = Path::new(MANIFEST_DIR).join("Cargo.toml/out.md");

        // Act / Assert
        assert!(validate_fetch(Jurisdiccion::Provincial, "ley-1", Some(&output)).is_err());
    }

    #[test]
    fn validate_fetch_output_is_a_directory_returns_error() {
        // Act
        let result = validate_fetch(
            Jurisdiccion::Provincial,
            "ley-1",
            Some(Path::new(MANIFEST_DIR)),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("is a directory")));
    }
}
