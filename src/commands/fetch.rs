//! `fetch`: prints one law's details and saves its text as Markdown.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use reqwest::Client;

use crate::http::{FetchError, fetch_html};
use crate::model::Jurisdiccion;
use crate::parse::{parse_law_page, parse_law_text};
use crate::site::{CRAWL_DELAY, SITE_ORIGIN, law_url};
use crate::validate::text_path;

/// A 404 means the slug is unknown to the site; any other failure is reported as it is.
fn describe_not_found(message: &'static str) -> impl FnOnce(FetchError) -> anyhow::Error {
    move |error| {
        if error.is_not_found() {
            anyhow::Error::new(error).context(message)
        } else {
            error.into()
        }
    }
}

pub(crate) async fn run_fetch(
    client: &Client,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    output: Option<&Path>,
) -> Result<()> {
    let text_file = text_path(ley, output);
    let html = fetch_html(client, &law_url(jurisdiccion, ley))
        .await
        .map_err(describe_not_found("law not found; check `--law`"))?;
    let page = parse_law_page(&html, jurisdiccion, ley, &text_file)?;

    // A second request follows, so honour the crawl delay.
    tokio::time::sleep(CRAWL_DELAY).await;
    let law_text = fetch_html(client, &format!("{SITE_ORIGIN}{}", page.text_path))
        .await
        .map_err(describe_not_found("law text not found; check `--law`"))?;
    tokio::fs::write(&text_file, parse_law_text(&law_text, jurisdiccion)?)
        .await
        .with_context(|| format!("failed to save {}", text_file.display()))?;

    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string(&page.details)?
    )
    .context("failed to write law details to stdout")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn describe_not_found_unknown_slug_adds_context() {
        // Arrange
        let error = FetchError::Status {
            url: "https://example.com/x".to_owned(),
            status: reqwest::StatusCode::NOT_FOUND,
            retry_after: None,
        };

        // Act
        let described = describe_not_found("law not found")(error);

        // Assert
        assert_eq!(described.to_string(), "law not found");
        assert!(format!("{described:#}").contains("404"));
    }

    #[test]
    fn describe_not_found_other_status_is_kept_as_it_is() {
        // Arrange
        let error = FetchError::Status {
            url: "https://example.com/x".to_owned(),
            status: reqwest::StatusCode::BAD_GATEWAY,
            retry_after: None,
        };

        // Act
        let described = describe_not_found("law not found")(error);

        // Assert
        assert!(!described.to_string().contains("law not found"));
    }
}
