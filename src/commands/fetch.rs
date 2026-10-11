//! `fetch`: prints one law's details and saves its text as Markdown.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chromiumoxide::page::Page;

use crate::browser::{fetch_html, wait_for_url_suffix};
use crate::model::Jurisdiccion;
use crate::parse::{parse_law_page, parse_law_text};
use crate::site::{CRAWL_DELAY, law_url};

const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";

// The "Ver norma" button leads to `<law url>/actualizacion`, the page holding the law text.
const VIEW_LAW_PATH_SUFFIX: &str = "/actualizacion";

/// Where the law text goes: `--output` when given, otherwise `<ley>.md` in the current directory.
fn text_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.md")), Path::to_path_buf)
}

pub(crate) async fn run_fetch(
    page: &Page,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    output: Option<&Path>,
) -> Result<()> {
    let text_file = text_path(ley, output);
    let html = fetch_html(page, &law_url(jurisdiccion, ley)).await?;
    let details = parse_law_page(&html, jurisdiccion, ley, &text_file)?;

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

    let law_text = page
        .content()
        .await
        .context("failed to read the law text page")?;
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
