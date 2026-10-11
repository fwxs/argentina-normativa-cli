//! `fetch`: prints one law's details and saves its text as Markdown.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};

use crate::http::fetch_html;
use crate::model::Jurisdiccion;
use crate::parse::{parse_law_page, parse_law_text, parse_law_text_link};
use crate::site::{CRAWL_DELAY, SITE_ORIGIN, law_url};

/// Where the law text goes: `--output` when given, otherwise `<ley>.md` in the current directory.
fn text_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.md")), Path::to_path_buf)
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
        .map_err(|error| {
            let status = error
                .downcast_ref::<reqwest::Error>()
                .and_then(reqwest::Error::status);
            if status == Some(StatusCode::NOT_FOUND) {
                error.context("law not found; check `--law`")
            } else {
                error
            }
        })?;
    let details = parse_law_page(&html, jurisdiccion, ley, &text_file)?;
    let law_text_path = parse_law_text_link(&html, jurisdiccion, ley)?;

    // A second request follows, so honour the crawl delay.
    tokio::time::sleep(CRAWL_DELAY).await;
    let law_text = fetch_html(client, &format!("{SITE_ORIGIN}{law_text_path}")).await?;
    std::fs::write(&text_file, parse_law_text(&law_text)?)
        .with_context(|| format!("failed to save {}", text_file.display()))?;

    writeln!(std::io::stdout(), "{}", serde_json::to_string(&details)?)
        .context("failed to write law details to stdout")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

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
