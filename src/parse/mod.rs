//! Pure HTML parsers: they turn saved or live page HTML into data and never touch the browser.

mod law;
mod national;
mod results;

use anyhow::{Result, anyhow};
use scraper::{Html, Selector};

pub(crate) use law::parse_law_page;
pub(crate) use national::{parse_agencies, parse_years};
pub(crate) use results::parse_results;

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|error| anyhow!("invalid selector `{css}`: {error}"))
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Option values of the select matched by `select_css`, skipping the empty placeholder.
pub(crate) fn parse_options(html: &str, select_css: &str) -> Result<Vec<String>> {
    let option_selector = selector(&format!("{select_css} option"))?;
    Ok(Html::parse_document(html)
        .select(&option_selector)
        .filter_map(|option| option.value().attr("value"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

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
}
