//! Scrapes "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
//! `list` prints the provinces of the search form; `query` prints matching provincial laws (or, with
//! `query national`, national norms) as JSON lines;
//! `fetch` prints one law's details as JSON and saves its text as a PDF.

mod cli;
mod model;
mod site;
mod validate;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::page::Page;
use futures::StreamExt;
use scraper::{Html, Selector};
use serde::Serialize;
use tokio::task::JoinHandle;

pub use crate::cli::{
    Cli, Command, ListScope, NationalArgs, NationalFilters, NationalList, QueryScope,
};
use crate::model::{Jurisdiccion, LawDetails, Normativa, ResultsPage};
use crate::site::{
    CRAWL_DELAY, LAW_TYPES, SEARCH_PATH, SITE_ORIGIN, TIPO_NORMA, law_url, national_search_url,
    parse_law_href, search_url,
};
use crate::validate::{validate_fetch, validate_national};

const PROVINCE_SELECTOR: &str = ".label";
const LAW_TITLE_SELECTOR: &str = "h1.h5";
const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";
const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";
// The "Ver norma" button leads to `<law url>/actualizacion`, the page holding the law text.
const VIEW_LAW_PATH_SUFFIX: &str = "/actualizacion";

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|error| anyhow!("invalid selector `{css}`: {error}"))
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Option values of the select matched by `select_css`, skipping the empty placeholder.
fn parse_options(html: &str, select_css: &str) -> Result<Vec<String>> {
    let option_selector = selector(&format!("{select_css} option"))?;
    Ok(Html::parse_document(html)
        .select(&option_selector)
        .filter_map(|option| option.value().attr("value"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Pages in the results counter: provincial "<total> normas encontradas en <pages> páginas" has the
/// page count in its second `span`; national has it only as text ("... en 25 páginas").
fn parse_total_pages(document: &Html) -> Result<usize> {
    let provincial_counter = selector("div.m-b-2 span.fw-bold")?;
    let national_counter = selector("div.infoleg-search-results-count")?;

    let national_pages = document
        .select(&national_counter)
        .next()
        .and_then(|counter| {
            let text = collapse_whitespace(&counter.text().collect::<String>());
            let words: Vec<&str> = text.split(' ').collect();
            words
                .windows(3)
                .find(|window| window[0] == "en" && window[2].starts_with("página"))
                .and_then(|window| window[1].parse().ok())
        });
    let provincial_pages = || {
        document
            .select(&provincial_counter)
            .nth(1)
            .and_then(|span| {
                collapse_whitespace(&span.text().collect::<String>())
                    .parse()
                    .ok()
            })
    };
    // Absent counter means nothing matched.
    Ok(national_pages.or_else(provincial_pages).unwrap_or(0))
}

/// "Resolución GENERAL 5911/2026" -> "Resolución GENERAL": the words before the number.
fn norm_type_from_title(titulo: &str) -> String {
    let words: Vec<&str> = titulo
        .split_whitespace()
        .take_while(|word| !word.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    if words.is_empty() {
        titulo.to_owned()
    } else {
        words.join(" ")
    }
}

fn parse_results(html: &str, provincia: Option<&str>) -> Result<ResultsPage> {
    let document = Html::parse_document(html);
    // Provincial rows sit in `#normas`; national ones in the div right after the results counter.
    let row_selector = selector("#normas tbody tr, .infoleg-search-results-count ~ div tbody tr")?;
    let link_selector = selector(r#"td[data-label="Normativa"] a"#)?;
    let agency_selector = selector(r#"td[data-label="Normativa"] p.small"#)?;
    let time_selector = selector("time[datetime]")?;
    let description_selector = selector(r#"td[data-label="Descripción"] p.small"#)?;

    let total_pages = parse_total_pages(&document)?;

    let rows = document
        .select(&row_selector)
        .filter_map(|row| {
            let link = row.select(&link_selector).next()?;
            let href = link.value().attr("href")?;
            let Some((jurisdiccion, ley)) = parse_law_href(href) else {
                tracing::warn!(href, "unrecognised normativa link");
                return None;
            };
            let titulo = collapse_whitespace(&link.text().collect::<String>());
            Some(Normativa {
                provincia: provincia.map(str::to_owned),
                jurisdiccion,
                tipo_norma: match jurisdiccion {
                    Jurisdiccion::Provincial => TIPO_NORMA.to_owned(),
                    Jurisdiccion::Nacional => norm_type_from_title(&titulo),
                },
                titulo,
                organismo: row
                    .select(&agency_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .find(|text| !text.is_empty()),
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

fn print_json_array<T: Serialize>(items: &[T], what: &str) -> Result<()> {
    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string_pretty(items)?
    )
    .with_context(|| format!("failed to write {what} to stdout"))
}

async fn run_list(page: &Page) -> Result<()> {
    let html = fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=provincial"),
    )
    .await?;
    print_json_array(
        &parse_options(&html, "select[name=provincia]")?,
        "provinces",
    )
}

// The national page repeats some selects in a second form; only the main search form counts.
const NATIONAL_FORM_SELECTOR: &str = "form#infoleg-normativa-search-form";

fn parse_agencies(html: &str) -> Result<Vec<String>> {
    parse_options(
        html,
        &format!("{NATIONAL_FORM_SELECTOR} select[name=dependencia]"),
    )
}

fn parse_years(html: &str) -> Result<Vec<u16>> {
    parse_options(html, &format!("{NATIONAL_FORM_SELECTOR} select[name=anio]"))?
        .iter()
        .map(|year| {
            year.parse()
                .with_context(|| format!("year option `{year}` is not a number"))
        })
        .collect()
}

async fn fetch_national_form(page: &Page) -> Result<String> {
    fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=nacional"),
    )
    .await
}

/// Lists of the national search form. `LawType` is a local constant (`main` answers it before
/// launching Chrome); it is kept here so the match stays exhaustive.
async fn run_list_national(page: &Page, what: NationalList) -> Result<()> {
    match what {
        NationalList::Agencies => print_json_array(
            &parse_agencies(&fetch_national_form(page).await?)?,
            "agencies",
        ),
        NationalList::Years => {
            print_json_array(&parse_years(&fetch_national_form(page).await?)?, "years")
        }
        NationalList::LawType => print_json_array(LAW_TYPES, "law types"),
    }
}

/// Pages to walk: all of them, unless the cap is lower.
fn pages_to_fetch(total_pages: usize, max_pages: usize) -> usize {
    total_pages.min(max_pages)
}

/// Walks the results pages of a search (at most `max_pages`), printing each law as a JSON line;
/// returns the row count. `url_for_page` takes a 0-based page index. The total page count comes
/// from the first page.
async fn run_search(
    page: &Page,
    provincia: Option<&str>,
    max_pages: usize,
    url_for_page: impl Fn(usize) -> Result<String>,
) -> Result<usize> {
    let mut page_index = 0;
    let mut last_page = 1;
    let mut written = 0usize;
    while page_index < last_page {
        if page_index > 0 {
            tokio::time::sleep(CRAWL_DELAY).await;
        }
        let html = fetch_html(page, &url_for_page(page_index)?).await?;
        let results = parse_results(&html, provincia)?;
        let total_pages = results.total_pages;
        if page_index == 0 {
            last_page = pages_to_fetch(total_pages, max_pages);
            if last_page < total_pages {
                tracing::warn!(
                    fetching = last_page,
                    total_pages,
                    "result truncated; raise --max-pages to fetch more"
                );
            }
        }
        for normativa in &results.rows {
            writeln!(std::io::stdout(), "{}", serde_json::to_string(normativa)?)
                .context("failed to write normativa to stdout")?;
            written += 1;
        }
        tracing::info!(
            provincia,
            page_number = page_index + 1,
            total_pages,
            "page fetched"
        );
        page_index += 1;
    }
    Ok(written)
}

async fn run_query(page: &Page, provincia: &str, query: &str) -> Result<()> {
    // The provincial site's `offset` is a 1-based page number.
    let written = run_search(page, Some(provincia), usize::MAX, |page_index| {
        search_url(provincia, query, page_index + 1)
    })
    .await?;
    if written == 0 {
        tracing::warn!(
            provincia,
            query,
            "no results; check the province name with `list`"
        );
    }
    Ok(())
}

async fn run_query_national(page: &Page, args: &NationalArgs) -> Result<()> {
    let max_pages = usize::try_from(args.max_pages).unwrap_or(usize::MAX);
    let written = run_search(page, None, max_pages, |page_index| {
        national_search_url(&args.filters, page_index)
    })
    .await?;
    if written == 0 {
        tracing::warn!("no results; check the filters (agency names must match exactly)");
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

/// Runs one parsed command: validates input, drives Chrome and prints the result to stdout.
///
/// Validation happens before Chrome starts so `--help` and bad arguments never launch a browser.
pub async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Fetch {
            jurisdiction, law, ..
        } => validate_fetch(*jurisdiction, law)?,
        Command::Query {
            scope: Some(QueryScope::National(args)),
            ..
        } => validate_national(&args.filters)?,
        Command::Query {
            scope: None,
            province,
            query,
        } if province.is_none() || query.is_none() => {
            bail!("--province and --query are required")
        }
        // Slugs are a local constant: answer without launching Chrome.
        Command::List {
            scope:
                Some(ListScope::National {
                    what: NationalList::LawType,
                }),
        } => return print_json_array(LAW_TYPES, "law types"),
        Command::List { .. } | Command::Query { .. } => {}
    }

    let (mut browser, handler_task, page) = launch_browser().await?;
    let outcome = match &cli.command {
        Command::List { scope: None } => run_list(&page).await,
        Command::List {
            scope: Some(ListScope::National { what }),
        } => run_list_national(&page, *what).await,
        Command::Query {
            scope: Some(QueryScope::National(args)),
            ..
        } => run_query_national(&page, args).await,
        Command::Query {
            province: Some(province),
            query: Some(query),
            ..
        } => run_query(&page, province, query).await,
        // Unreachable: clap requires both flags unless a subcommand is given, and the check above
        // runs before launching Chrome.
        Command::Query { .. } => bail!("--province and --query are required"),
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
    const NATIONAL_RESULTS_FIXTURE: &str = include_str!("../tests/fixtures/results_national.html");
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
    fn parse_results_national_fixture_returns_five_rows_over_212_pages() -> Result<()> {
        // Act
        let results = parse_results(NATIONAL_RESULTS_FIXTURE, None)?;

        // Assert
        assert_eq!(results.total_pages, 212);
        assert_eq!(results.rows.len(), 5);
        let first = &results.rows[0];
        assert_eq!(first.provincia, None);
        assert_eq!(first.jurisdiccion, Jurisdiccion::Nacional);
        assert_eq!(first.tipo_norma, "Ley");
        assert_eq!(first.titulo, "Ley 27826");
        assert_eq!(first.ley, "norma-431078");
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/nacional/norma-431078")
        );
        assert_eq!(
            first.organismo.as_deref(),
            Some("HONORABLE CONGRESO DE LA NACION ARGENTINA")
        );
        assert_eq!(first.fecha_publicacion.as_deref(), Some("2026-10-09"));
        assert_eq!(first.descripcion.len(), 3);
        assert_eq!(results.rows[1].tipo_norma, "Resolución GENERAL");
        Ok(())
    }

    #[test]
    fn parse_results_national_zero_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">0</span> norma encontrada</div>
            <table><tbody><tr><td colspan="3">No se encontraron resultados</td></tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

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
    fn parse_results_provincial_fixture_rows_are_all_provincial_without_organismo() -> Result<()> {
        // Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.rows.len(), 50);
        for row in &results.rows {
            assert_eq!(row.jurisdiccion, Jurisdiccion::Provincial);
            assert_eq!(row.tipo_norma, "Ley");
            assert!(!serde_json::to_string(row)?.contains("organismo"));
        }
        Ok(())
    }

    #[test]
    fn parse_results_ignores_rows_of_unrelated_tables() -> Result<()> {
        // Arrange
        let html = r#"<table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.rows, vec![]);
        Ok(())
    }

    #[test]
    fn parse_results_national_single_result_page_counts_one_page() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">1</span> norma encontrada en 1 página</div>
            <div class=""><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.total_pages, 1);
        assert_eq!(results.rows.len(), 1);
        Ok(())
    }

    #[test]
    fn norm_type_from_title_drops_number_and_year() {
        // Act / Assert
        assert_eq!(
            norm_type_from_title("Resolución GENERAL 5911/2026"),
            "Resolución GENERAL"
        );
        assert_eq!(
            norm_type_from_title("Decisión Administrativa 3/2024"),
            "Decisión Administrativa"
        );
        assert_eq!(norm_type_from_title("27826"), "27826");
    }

    #[test]
    fn parse_options_select_with_placeholder_skips_empty_value() -> Result<()> {
        // Arrange
        let html = r#"<select name="provincia"><option value="">-</option>
            <option value="Córdoba">Córdoba</option></select>"#;

        // Act
        let options = parse_options(html, "select[name=provincia]")?;

        // Assert
        assert_eq!(options, vec!["Córdoba".to_owned()]);
        Ok(())
    }

    #[test]
    fn parse_agencies_national_fixture_ignores_placeholder_and_second_form() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let agencies = parse_agencies(&html)?;

        // Assert
        assert_eq!(
            agencies,
            vec![
                "1RA. COM. NAC. DE SALARIOS IND. DEL VESTIDO TRABAJO A DOMIC.".to_owned(),
                "ADM. GRAL. DEL SERVICIO NACIONAL DE SANIDAD ANIMAL".to_owned(),
                "ADMINISTRACION ADUANA BARILOCHE".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn parse_years_national_fixture_keeps_site_order_as_numbers() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let years = parse_years(&html)?;

        // Assert
        assert_eq!(years, vec![2026, 2025, 2024]);
        Ok(())
    }

    #[test]
    fn parse_years_non_numeric_option_returns_error() {
        // Arrange
        let html = r#"<form id="infoleg-normativa-search-form"><select name="anio">
            <option value="dos mil">x</option></select></form>"#;

        // Act
        let result = parse_years(html);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn pages_to_fetch_caps_total_pages() {
        // Act / Assert
        assert_eq!(pages_to_fetch(212, 20), 20);
        assert_eq!(pages_to_fetch(3, 20), 3);
        assert_eq!(pages_to_fetch(0, 20), 0);
        assert_eq!(pages_to_fetch(5, usize::MAX), 5);
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
