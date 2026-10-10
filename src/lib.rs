//! Scrapes "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
//! `list` prints the provinces of the search form; `query` prints matching provincial laws (or, with
//! `query national`, national norms) as JSON lines;
//! `fetch` prints one law's details as JSON and saves its text as a PDF.

mod browser;
mod cli;
mod model;
mod output;
mod parse;
mod site;
mod validate;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::page::Page;

use crate::browser::{fetch_html, launch_browser, wait_for_url_suffix};
pub use crate::cli::{
    Cli, Command, ListScope, NationalArgs, NationalFilters, NationalList, QueryScope,
};
use crate::model::Jurisdiccion;
use crate::output::print_json_array;
use crate::parse::{parse_agencies, parse_law_page, parse_options, parse_results, parse_years};
use crate::site::{
    CRAWL_DELAY, LAW_TYPES, SEARCH_PATH, SITE_ORIGIN, law_url, national_search_url, search_url,
};
use crate::validate::{validate_fetch, validate_national};

const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";
// The "Ver norma" button leads to `<law url>/actualizacion`, the page holding the law text.
const VIEW_LAW_PATH_SUFFIX: &str = "/actualizacion";

/// Where the law's PDF goes: `--output` when given, otherwise `<ley>.pdf` in the current directory.
fn pdf_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.pdf")), Path::to_path_buf)
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

    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn pages_to_fetch_caps_total_pages() {
        // Act / Assert
        assert_eq!(pages_to_fetch(212, 20), 20);
        assert_eq!(pages_to_fetch(3, 20), 3);
        assert_eq!(pages_to_fetch(0, 20), 0);
        assert_eq!(pages_to_fetch(5, usize::MAX), 5);
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
