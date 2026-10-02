// Scrapes provincial "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
// `list` prints the provinces of the search form; `query` prints matching laws as JSON lines.

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use clap::{Parser, Subcommand};
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
}

#[derive(Debug, Serialize, PartialEq)]
struct Normativa {
    provincia: String,
    tipo_norma: String,
    titulo: String,
    url: String,
    fecha_publicacion: Option<String>,
    descripcion: Vec<String>,
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

fn parse_results(html: &str, provincia: &str) -> Result<ResultsPage> {
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
            Some(Normativa {
                provincia: provincia.to_owned(),
                tipo_norma: TIPO_NORMA.to_owned(),
                titulo: collapse_whitespace(&link.text().collect::<String>()),
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
        let results = parse_results(&html, provincia)?;
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

#[tokio::main]
async fn main() -> Result<()> {
    // Parse first so `--help` and argument errors never start Chrome.
    let cli = Cli::parse();

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

    #[test]
    fn parse_results_buenos_aires_laws_page_returns_fifty_rows_over_three_pages() -> Result<()> {
        // Arrange / Act
        let results = parse_results(RESULTS_FIXTURE, "Buenos Aires")?;

        // Assert
        assert_eq!(results.total_pages, 3);
        assert_eq!(results.rows.len(), 50);
        let first = &results.rows[0];
        assert_eq!(first.tipo_norma, "Ley");
        assert!(
            first
                .url
                .starts_with("https://www.argentina.gob.ar/normativa/provincial/")
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
        let results = parse_results(html, "Chaco")?;

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
}
