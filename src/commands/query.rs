//! `query`: walks the paginated search results and prints them as JSON lines.

use std::io::Write;

use anyhow::{Context, Result};
use reqwest::Client;

use crate::cli::{NationalArgs, ProvinceArgs};
use crate::http::fetch_html;
use crate::parse::parse_results;
use crate::site::{CRAWL_DELAY, national_search_url, search_url};

/// Pages to walk: all of them, unless the cap is lower.
fn pages_to_fetch(total_pages: usize, max_pages: usize) -> usize {
    total_pages.min(max_pages)
}

/// Walks the results pages of a search (at most `max_pages`), printing each law as a JSON line;
/// returns the row count. `url_for_page` takes a 0-based page index. The total page count comes
/// from the first page.
async fn run_search(
    client: &Client,
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
        // Rows are printed as they arrive, so say where a failure cut the output short.
        let page_context = || {
            format!(
                "failed on page {}; {written} rows were already printed",
                page_index + 1
            )
        };
        let html = fetch_html(client, &url_for_page(page_index)?)
            .await
            .with_context(page_context)?;
        let results = parse_results(&html, provincia).with_context(page_context)?;
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

pub(crate) async fn run_query_provinces(client: &Client, args: &ProvinceArgs) -> Result<()> {
    let written = run_search(client, Some(&args.province), usize::MAX, |page_index| {
        search_url(args, page_index + 1)
    })
    .await?;
    if written == 0 {
        tracing::warn!(
            provincia = args.province.as_str(),
            "no results; check the province name with `list` and the filters"
        );
    }
    Ok(())
}

pub(crate) async fn run_query_national(client: &Client, args: &NationalArgs) -> Result<()> {
    let max_pages = usize::try_from(args.max_pages).unwrap_or(usize::MAX);
    let written = run_search(client, None, max_pages, |page_index| {
        national_search_url(&args.filters, page_index)
    })
    .await?;
    if written == 0 {
        tracing::warn!("no results; check the filters (agency names must match exactly)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn pages_to_fetch_caps_total_pages() {
        // Act / Assert
        assert_eq!(pages_to_fetch(212, 20), 20);
        assert_eq!(pages_to_fetch(3, 20), 3);
        assert_eq!(pages_to_fetch(0, 20), 0);
        assert_eq!(pages_to_fetch(5, usize::MAX), 5);
    }
}
