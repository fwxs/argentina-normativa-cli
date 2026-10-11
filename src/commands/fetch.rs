//! `fetch`: prints one law's details and saves its text as Markdown.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::Client;

use crate::http::{FetchError, fetch_html};
use crate::model::Jurisdiccion;
use crate::parse::{parse_law_page, parse_law_text, parse_law_text_link};
use crate::site::{CRAWL_DELAY, SITE_ORIGIN, law_url};

/// Where the law text goes: `--output` when given, otherwise `<ley>.md` in the current directory.
fn text_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.md")), Path::to_path_buf)
}

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
    let details = parse_law_page(&html, jurisdiccion, ley, &text_file)?;
    let law_text_path = parse_law_text_link(&html, jurisdiccion, ley)?;

    // A second request follows, so honour the crawl delay.
    tokio::time::sleep(CRAWL_DELAY).await;
    let law_text = fetch_html(client, &format!("{SITE_ORIGIN}{law_text_path}"))
        .await
        .map_err(describe_not_found("law text not found; check `--law`"))?;
    tokio::fs::write(&text_file, parse_law_text(&law_text)?)
        .await
        .with_context(|| format!("failed to save {}", text_file.display()))?;

    writeln!(std::io::stdout(), "{}", serde_json::to_string(&details)?)
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
        };

        // Act
        let described = describe_not_found("law not found")(error);

        // Assert
        assert!(!described.to_string().contains("law not found"));
    }

    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn text_path_without_output_defaults_to_law_slug() {
        // Act
        let path = text_path(LAW_SLUG, None);

        // Assert
        assert_eq!(path, PathBuf::from(format!("{LAW_SLUG}.md")));
    }

    #[test]
    fn text_path_with_output_uses_given_path() {
        // Act
        let path = text_path(LAW_SLUG, Some(Path::new("laws/custom.md")));

        // Assert
        assert_eq!(path, PathBuf::from("laws/custom.md"));
    }
}
