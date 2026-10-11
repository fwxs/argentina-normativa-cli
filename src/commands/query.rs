//! `query`: walks the paginated search results and prints them as JSON lines.

use std::io::Write;

use anyhow::{Context, Result, bail};
use reqwest::Client;

use crate::cli::{NationalArgs, ProvinceArgs};
use crate::http::fetch_html;
use crate::parse::parse_results;
use crate::site::{CRAWL_DELAY, national_search_url, search_url};

/// Pages to walk: all of them, unless the cap is lower.
fn pages_to_fetch(total_pages: usize, max_pages: usize) -> usize {
    total_pages.min(max_pages)
}

/// Rejects a page that cannot be right, so a broken page counter or an error page that answered
/// 200 can't end the crawl early with exit code 0. Only the first page may be empty, and only
/// when the counter says there is nothing.
fn check_page(page_index: usize, total_pages: usize, row_count: usize) -> Result<()> {
    match (page_index, total_pages, row_count) {
        (0, 0, 0) => Ok(()),
        (0, 0, _) => bail!("found rows but no page count; the results counter may have changed"),
        (0, _, 0) => bail!("the counter announces {total_pages} pages but the page has no rows"),
        (1.., _, 0) => bail!("the page has no rows"),
        _ => Ok(()),
    }
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
        check_page(page_index, total_pages, results.rows.len()).with_context(page_context)?;
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
            "no results; check the province name with `list provinces` and the filters"
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
    use crate::http::test_support::{html_response, serve, test_client};

    /// A national results page with one row, in the markup `parse_results` reads.
    fn results_page(total_pages: usize, row_number: usize) -> String {
        html_response(&format!(
            r#"<div class="infoleg-search-results-count">2 normas encontradas en {total_pages} páginas</div>
            <div><table><tbody><tr><td data-label="Normativa">
            <a href="/normativa/nacional/norma-{row_number}">Decreto {row_number}/2026</a>
            </td></tr></tbody></table></div>"#
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn run_search_two_pages_returns_the_rows_of_both() -> Result<()> {
        // Arrange
        let url = serve(vec![results_page(2, 1), results_page(2, 2)]).await?;

        // Act
        let written = run_search(&test_client()?, None, usize::MAX, |_| Ok(url.clone())).await?;

        // Assert
        assert_eq!(written, 2);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn run_search_max_pages_one_requests_only_the_first_page() -> Result<()> {
        // Arrange: the server answers a single connection, so a request for page 2 would fail.
        let url = serve(vec![results_page(2, 1)]).await?;

        // Act
        let written = run_search(&test_client()?, None, 1, |_| Ok(url.clone())).await?;

        // Assert
        assert_eq!(written, 1);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn run_search_failing_second_page_reports_page_and_printed_rows() -> Result<()> {
        // Arrange
        let not_found = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let url = serve(vec![results_page(2, 1), not_found.to_owned()]).await?;

        // Act
        let result = run_search(&test_client()?, None, usize::MAX, |_| Ok(url.clone())).await;

        // Assert
        let message = format!("{:#}", result.expect_err("page 2 answers 404"));
        assert!(
            message.contains("failed on page 2; 1 rows were already printed"),
            "{message}"
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn run_search_empty_second_page_returns_error() -> Result<()> {
        // Arrange: page 2 answers 200 with a page that has no rows, like an error page would.
        let empty_page = html_response("<html><body>Algo salio mal</body></html>");
        let url = serve(vec![results_page(3, 1), empty_page]).await?;

        // Act
        let result = run_search(&test_client()?, None, usize::MAX, |_| Ok(url.clone())).await;

        // Assert
        let message = format!("{:#}", result.expect_err("page 2 has no rows"));
        assert!(
            message.contains("failed on page 2; 1 rows were already printed"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn check_page_first_page_without_results_is_ok() {
        // Act / Assert
        assert!(check_page(0, 0, 0).is_ok());
    }

    #[test]
    fn check_page_rows_but_no_page_count_returns_error() {
        // Act / Assert
        assert!(check_page(0, 0, 3).is_err());
    }

    #[test]
    fn check_page_first_page_with_count_but_no_rows_returns_error() {
        // Act / Assert
        assert!(check_page(0, 4, 0).is_err());
    }

    #[test]
    fn check_page_later_page_without_rows_returns_error() {
        // Act / Assert
        assert!(check_page(1, 4, 0).is_err());
        assert!(check_page(1, 4, 50).is_ok());
    }

    #[test]
    fn pages_to_fetch_caps_total_pages() {
        // Act / Assert
        assert_eq!(pages_to_fetch(212, 20), 20);
        assert_eq!(pages_to_fetch(3, 20), 3);
        assert_eq!(pages_to_fetch(0, 20), 0);
        assert_eq!(pages_to_fetch(5, usize::MAX), 5);
    }
}
