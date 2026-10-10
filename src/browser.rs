//! Headless Chrome driving (chromiumoxide).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use tokio::task::JoinHandle;

pub(crate) async fn fetch_html(page: &Page, url: &str) -> Result<String> {
    page.goto(url)
        .await
        .with_context(|| format!("failed to open {url}"))?;
    page.content()
        .await
        .with_context(|| format!("failed to read content of {url}"))
}

pub(crate) async fn launch_browser() -> Result<(Browser, JoinHandle<()>, Page)> {
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

// The click only starts the navigation, so poll until the page url shows it happened.
pub(crate) async fn wait_for_url_suffix(page: &Page, suffix: &str) -> Result<()> {
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
