// Scrapes provincial "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
// `list` prints the provinces of the search form; `query` prints matching laws as JSON lines;
// `fetch` prints one law's details as JSON and saves its text as a PDF.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::page::Page;
use clap::{Parser, Subcommand, ValueEnum};
use futures::StreamExt;
use scraper::{Html, Selector};
use serde::Serialize;
use tokio::task::JoinHandle;
use url::Url;

const SITE_ORIGIN: &str = "https://www.argentina.gob.ar";
const SEARCH_PATH: &str = "/normativa";
const PAGE_SIZE: &str = "50";
// The site's provincial search only accepts this norm type (the form field is disabled with it).
const TIPO_NORMA: &str = "Ley";
// robots.txt asks for `Crawl-delay: 10`.
const CRAWL_DELAY: Duration = Duration::from_secs(10);

const PROVINCE_SELECTOR: &str = ".label";
const LAW_TITLE_SELECTOR: &str = "h1.h5";
const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";
const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";
// The "Ver norma" button leads to `<law url>/actualizacion`, the page holding the law text.
const VIEW_LAW_PATH_SUFFIX: &str = "/actualizacion";

/// Scraper for provincial laws published on argentina.gob.ar/normativa.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the provinces of the "Elegí una provincia" select box as a JSON array.
    List,
    /// Search the laws of one province and print them as JSON lines on stdout.
    Query {
        /// Province name exactly as printed by `list`.
        #[arg(long)]
        province: String,
        /// Keywords for "Buscá por palabras clave", e.g. "impuesto tasa".
        #[arg(long)]
        query: String,
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

#[derive(Debug, Serialize, PartialEq)]
struct LawDetails {
    provincia: String,
    jurisdiccion: Jurisdiccion,
    titulo: String,
    ley: String,
    estado: Option<String>,
    url: String,
    pdf: String,
}

#[derive(Debug, Serialize, PartialEq)]
struct Normativa {
    provincia: Option<String>,
    jurisdiccion: Jurisdiccion,
    tipo_norma: String,
    titulo: String,
    // Last path segment of the law url, e.g. "ley-11035-123456789-0abc-defg-373-0000svorpyel".
    ley: String,
    url: String,
    fecha_publicacion: Option<String>,
    descripcion: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Jurisdiccion {
    Provincial,
    Nacional,
}

impl Jurisdiccion {
    fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "provincial" => Some(Self::Provincial),
            "nacional" => Some(Self::Nacional),
            _ => None,
        }
    }

    fn path_segment(self) -> &'static str {
        match self {
            Self::Provincial => "provincial",
            Self::Nacional => "nacional",
        }
    }
}

/// `--law` ends up in a url path and a file name, so only slug characters are allowed.
fn validate_law_slug(ley: &str) -> Result<()> {
    if ley.is_empty() || !ley.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid law `{ley}`: expected a slug of letters, digits and dashes");
    }
    Ok(())
}

fn validate_fetch(jurisdiccion: Jurisdiccion, ley: &str) -> Result<()> {
    match jurisdiccion {
        Jurisdiccion::Provincial => validate_law_slug(ley),
        Jurisdiccion::Nacional => bail!("fetching `nacional` laws is not supported yet"),
    }
}

/// Splits a law link `/normativa/<jurisdiccion>/<ley>` into its jurisdiction and law slug.
fn parse_law_href(href: &str) -> Option<(Jurisdiccion, &str)> {
    let (segment, ley) = href
        .strip_prefix(SEARCH_PATH)?
        .strip_prefix('/')?
        .split_once('/')?;
    if ley.is_empty() || ley.contains('/') {
        return None;
    }
    Some((Jurisdiccion::from_path_segment(segment)?, ley))
}

#[derive(Debug, PartialEq)]
struct ResultsPage {
    total_pages: usize,
    rows: Vec<Normativa>,
}

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|error| anyhow!("invalid selector `{css}`: {error}"))
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Option values of `<select name="...">`, skipping the empty placeholder.
fn parse_options(html: &str, select_name: &str) -> Result<Vec<String>> {
    let option_selector = selector(&format!("select[name={select_name}] option"))?;
    Ok(Html::parse_document(html)
        .select(&option_selector)
        .filter_map(|option| option.value().attr("value"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect())
}

fn parse_results(html: &str, provincia: Option<&str>) -> Result<ResultsPage> {
    let document = Html::parse_document(html);
    let row_selector = selector("#normas tbody tr")?;
    let link_selector = selector(r#"td[data-label="Normativa"] a"#)?;
    let time_selector = selector("time[datetime]")?;
    let description_selector = selector(r#"td[data-label="Descripción"] p.small"#)?;
    let counter_selector = selector("div.m-b-2 span.fw-bold")?;

    // Counter reads "<total> normas encontradas en <pages> páginas"; absent when nothing matched.
    let total_pages = document
        .select(&counter_selector)
        .nth(1)
        .and_then(|span| {
            collapse_whitespace(&span.text().collect::<String>())
                .parse()
                .ok()
        })
        .unwrap_or(0);

    let rows = document
        .select(&row_selector)
        .filter_map(|row| {
            let link = row.select(&link_selector).next()?;
            let href = link.value().attr("href")?;
            let Some((jurisdiccion, ley)) = parse_law_href(href) else {
                tracing::warn!(href, "unrecognised normativa link");
                return None;
            };
            Some(Normativa {
                provincia: provincia.map(str::to_owned),
                jurisdiccion,
                tipo_norma: TIPO_NORMA.to_owned(),
                titulo: collapse_whitespace(&link.text().collect::<String>()),
                ley: ley.to_owned(),
                url: format!("{SITE_ORIGIN}{href}"),
                fecha_publicacion: row
                    .select(&time_selector)
                    .next()
                    .and_then(|time| time.value().attr("datetime"))
                    .map(str::to_owned),
                descripcion: row
                    .select(&description_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .filter(|text| !text.is_empty())
                    .collect(),
            })
        })
        .collect();

    Ok(ResultsPage { total_pages, rows })
}

/// Where the law's PDF goes: `--output` when given, otherwise `<ley>.pdf` in the current directory.
fn pdf_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.pdf")), Path::to_path_buf)
}

fn parse_law_page(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    pdf: &Path,
) -> Result<LawDetails> {
    let document = Html::parse_document(html);
    let text_of = |css: &str| -> Result<Option<String>> {
        Ok(document
            .select(&selector(css)?)
            .next()
            .map(|element| collapse_whitespace(&element.text().collect::<String>()))
            .filter(|text| !text.is_empty()))
    };
    let (Some(provincia), Some(titulo)) =
        (text_of(PROVINCE_SELECTOR)?, text_of(LAW_TITLE_SELECTOR)?)
    else {
        bail!("law not found: no province or title on the page; check `--law`");
    };
    Ok(LawDetails {
        provincia,
        jurisdiccion,
        titulo,
        ley: ley.to_owned(),
        estado: text_of(LAW_STATUS_SELECTOR)?,
        url: law_url(jurisdiccion, ley),
        pdf: pdf.display().to_string(),
    })
}

fn law_url(jurisdiccion: Jurisdiccion, ley: &str) -> String {
    format!(
        "{SITE_ORIGIN}{SEARCH_PATH}/{}/{ley}",
        jurisdiccion.path_segment()
    )
}

fn search_url(provincia: &str, query: &str, page_number: usize) -> Result<String> {
    let page_number = page_number.to_string();
    let url = Url::parse_with_params(
        &format!("{SITE_ORIGIN}{SEARCH_PATH}"),
        [
            ("provincia", provincia),
            ("jurisdiccion", "provincial"),
            ("tipo_norma", TIPO_NORMA),
            ("texto", query),
            ("limit", PAGE_SIZE),
            // The site's `offset` is a 1-based page number.
            ("offset", page_number.as_str()),
        ],
    )
    .context("failed to build search url")?;
    Ok(url.into())
}

async fn fetch_html(page: &Page, url: &str) -> Result<String> {
    page.goto(url)
        .await
        .with_context(|| format!("failed to open {url}"))?;
    page.content()
        .await
        .with_context(|| format!("failed to read content of {url}"))
}

async fn launch_browser() -> Result<(Browser, JoinHandle<()>, Page)> {
    let browser_config = BrowserConfig::builder()
        .build()
        .map_err(|error| anyhow!("invalid browser config: {error}"))?;
    let (browser, mut handler) = Browser::launch(browser_config)
        .await
        .context("failed to launch headless Chrome")?;
    // Chrome emits CDP messages chromiumoxide can't decode; those errors are not fatal, keep polling.
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser
        .new_page("about:blank")
        .await
        .context("failed to open browser page")?;
    Ok((browser, handler_task, page))
}

async fn run_list(page: &Page) -> Result<()> {
    let html = fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=provincial"),
    )
    .await?;
    let provinces = parse_options(&html, "provincia")?;
    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string_pretty(&provinces)?
    )
    .context("failed to write provinces to stdout")
}

async fn run_query(page: &Page, provincia: &str, query: &str) -> Result<()> {
    let mut page_number = 1;
    let mut total_pages = 1;
    let mut written = 0usize;
    while page_number <= total_pages {
        if page_number > 1 {
            tokio::time::sleep(CRAWL_DELAY).await;
        }
        let html = fetch_html(page, &search_url(provincia, query, page_number)?).await?;
        let results = parse_results(&html, Some(provincia))?;
        if page_number == 1 {
            total_pages = results.total_pages;
        }
        for normativa in &results.rows {
            writeln!(std::io::stdout(), "{}", serde_json::to_string(normativa)?)
                .context("failed to write normativa to stdout")?;
            written += 1;
        }
        tracing::info!(provincia, page_number, total_pages, "page fetched");
        page_number += 1;
    }
    if written == 0 {
        tracing::warn!(
            provincia,
            query,
            "no results; check the province name with `list`"
        );
    }
    Ok(())
}

// The click only starts the navigation, so poll until the page url shows it happened.
async fn wait_for_url_suffix(page: &Page, suffix: &str) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    const MAX_POLLS: u32 = 60;
    for _ in 0..MAX_POLLS {
        let current = page.url().await.context("failed to read page url")?;
        if current.as_deref().is_some_and(|url| url.ends_with(suffix)) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!("page url did not end with `{suffix}` after {MAX_POLLS} polls")
}

async fn run_fetch(
    page: &Page,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    output: Option<&Path>,
) -> Result<()> {
    let pdf = pdf_path(ley, output);
    let html = fetch_html(page, &law_url(jurisdiccion, ley)).await?;
    let details = parse_law_page(&html, jurisdiccion, ley, &pdf)?;

    // A second request follows, so honour the crawl delay.
    tokio::time::sleep(CRAWL_DELAY).await;
    page.find_element(VIEW_LAW_BUTTON_SELECTOR)
        .await
        .context("failed to find the \"Ver norma\" button")?
        .click()
        .await
        .context("failed to click the \"Ver norma\" button")?;
    wait_for_url_suffix(page, VIEW_LAW_PATH_SUFFIX)
        .await
        .context("clicking \"Ver norma\" did not open the law text page")?;
    page.wait_for_navigation()
        .await
        .context("failed to load the law text page")?;

    let pdf_params = PrintToPdfParams {
        print_background: Some(true),
        ..Default::default()
    };
    page.save_pdf(pdf_params, &pdf)
        .await
        .with_context(|| format!("failed to save {}", pdf.display()))?;

    writeln!(std::io::stdout(), "{}", serde_json::to_string(&details)?)
        .context("failed to write law details to stdout")
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse first so `--help` and argument errors never start Chrome.
    let cli = Cli::parse();
    if let Command::Fetch {
        jurisdiction, law, ..
    } = &cli.command
    {
        validate_fetch(*jurisdiction, law)?;
    }

    // stdout carries data only; logs go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,chromiumoxide=error".into()),
        )
        .init();

    let (mut browser, handler_task, page) = launch_browser().await?;
    let outcome = match &cli.command {
        Command::List => run_list(&page).await,
        Command::Query { province, query } => run_query(&page, province, query).await,
        Command::Fetch {
            jurisdiction,
            law,
            output,
        } => run_fetch(&page, *jurisdiction, law, output.as_deref()).await,
    };
    // Close before propagating the command error so Chrome never outlives us.
    browser.close().await.context("failed to close browser")?;
    handler_task
        .await
        .context("browser handler task panicked")?;
    outcome
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const RESULTS_FIXTURE: &str = include_str!("../tests/fixtures/results.html");
    const LAW_FIXTURE: &str = include_str!("../tests/fixtures/law.html");
    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn parse_results_buenos_aires_laws_page_returns_fifty_rows_over_three_pages() -> Result<()> {
        // Arrange / Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.total_pages, 3);
        assert_eq!(results.rows.len(), 50);
        let first = &results.rows[0];
        assert_eq!(first.provincia.as_deref(), Some("Buenos Aires"));
        assert_eq!(first.jurisdiccion, Jurisdiccion::Provincial);
        assert_eq!(first.tipo_norma, "Ley");
        assert!(first.ley.starts_with("ley-"));
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/provincial/{}", first.ley)
        );
        assert!(!first.titulo.is_empty());
        assert!(first.fecha_publicacion.is_some());
        Ok(())
    }

    #[test]
    fn parse_results_no_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = "<html><body><p>No se encontraron normas</p></body></html>";

        // Act
        let results = parse_results(html, Some("Chaco"))?;

        // Assert
        assert_eq!(
            results,
            ResultsPage {
                total_pages: 0,
                rows: vec![]
            }
        );
        Ok(())
    }

    #[test]
    fn parse_results_without_province_serialises_null_province() -> Result<()> {
        // Arrange
        let html = r#"<div id="normas"><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/ley-1-abc"> Ley 1 </a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;
        let json = serde_json::to_string(&results.rows[0])?;

        // Assert
        assert!(json.contains(r#""provincia":null"#));
        assert!(json.contains(r#""jurisdiccion":"nacional""#));
        assert!(json.contains(r#""ley":"ley-1-abc""#));
        Ok(())
    }

    #[test]
    fn parse_law_href_provincial_link_returns_jurisdiction_and_slug() {
        // Act
        let parsed =
            parse_law_href("/normativa/provincial/ley-11035-123456789-0abc-defg-373-0000svorpyel");

        // Assert
        assert_eq!(
            parsed,
            Some((
                Jurisdiccion::Provincial,
                "ley-11035-123456789-0abc-defg-373-0000svorpyel"
            ))
        );
    }

    #[test]
    fn parse_law_href_nacional_link_returns_nacional() {
        // Act
        let parsed = parse_law_href("/normativa/nacional/ley-27078-abc");

        // Assert
        assert_eq!(parsed, Some((Jurisdiccion::Nacional, "ley-27078-abc")));
    }

    #[test]
    fn parse_law_href_unknown_jurisdiction_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/municipal/ley-1"), None);
    }

    #[test]
    fn parse_law_href_missing_slug_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/provincial/"), None);
        assert_eq!(parse_law_href("/normativa/provincial"), None);
        assert_eq!(parse_law_href("/otra/provincial/ley-1"), None);
    }

    #[test]
    fn parse_options_select_with_placeholder_skips_empty_value() -> Result<()> {
        // Arrange
        let html = r#"<select name="provincia"><option value="">-</option>
            <option value="Córdoba">Córdoba</option></select>"#;

        // Act
        let options = parse_options(html, "provincia")?;

        // Assert
        assert_eq!(options, vec!["Córdoba".to_owned()]);
        Ok(())
    }

    #[test]
    fn search_url_accented_province_is_percent_encoded() -> Result<()> {
        // Act
        let url = search_url("Río Negro", "impuesto tasa", 2)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?provincia=R%C3%ADo+Negro&jurisdiccion=provincial\
             &tipo_norma=Ley&texto=impuesto+tasa&limit=50&offset=2"
        );
        Ok(())
    }

    #[test]
    fn cli_list_without_arguments_parses() {
        // Act
        let cli = Cli::try_parse_from(["normativa-scraper", "list"]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli {
                command: Command::List
            })
        ));
    }

    #[test]
    fn cli_query_with_flags_parses_province_and_query() {
        // Act
        let cli = Cli::try_parse_from([
            "normativa-scraper",
            "query",
            "--province",
            "Córdoba",
            "--query",
            "impuesto tasa",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { province, query } })
                if province == "Córdoba" && query == "impuesto tasa"
        ));
    }

    #[test]
    fn cli_query_without_province_fails() {
        // Act
        let cli = Cli::try_parse_from(["normativa-scraper", "query", "--query", "impuesto"]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn parse_law_page_buenos_aires_fixture_returns_province_title_status() -> Result<()> {
        // Act
        let details = parse_law_page(
            LAW_FIXTURE,
            Jurisdiccion::Provincial,
            LAW_SLUG,
            Path::new("out/ley.pdf"),
        )?;

        // Assert
        assert_eq!(
            details,
            LawDetails {
                provincia: "Buenos Aires".to_owned(),
                jurisdiccion: Jurisdiccion::Provincial,
                titulo: "Ley 14709".to_owned(),
                ley: LAW_SLUG.to_owned(),
                estado: Some("Vigente, de alcance general".to_owned()),
                url: format!("{SITE_ORIGIN}/normativa/provincial/{LAW_SLUG}"),
                pdf: "out/ley.pdf".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn parse_law_page_page_without_label_returns_not_found_error() {
        // Arrange
        let html = "<html><body><h1>Página no encontrada</h1></body></html>";

        // Act
        let result = parse_law_page(
            html,
            Jurisdiccion::Provincial,
            "ley-1",
            Path::new("ley-1.pdf"),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law not found")));
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
    fn validate_fetch_nacional_returns_unsupported_error() {
        // Act
        let result = validate_fetch(Jurisdiccion::Nacional, "ley-1");

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("not supported yet")));
    }

    #[test]
    fn cli_fetch_with_flags_parses_jurisdiction_and_law() {
        // Act
        let cli = Cli::try_parse_from([
            "normativa-scraper",
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
            "normativa-scraper",
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
        let cli =
            Cli::try_parse_from(["normativa-scraper", "fetch", "--jurisdiction", "provincial"]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_with_output_flag_parses_file_path() {
        // Act
        let cli = Cli::try_parse_from([
            "normativa-scraper",
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

    #[test]
    fn pdf_path_without_output_defaults_to_law_slug() {
        // Act
        let path = pdf_path(LAW_SLUG, None);

        // Assert
        assert_eq!(path, PathBuf::from(format!("{LAW_SLUG}.pdf")));
    }

    #[test]
    fn pdf_path_with_output_uses_given_path() {
        // Act
        let path = pdf_path(LAW_SLUG, Some(Path::new("laws/custom.pdf")));

        // Assert
        assert_eq!(path, PathBuf::from("laws/custom.pdf"));
    }
}
