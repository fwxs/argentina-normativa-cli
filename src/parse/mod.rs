//! Pure HTML parsers: they turn saved or live page HTML into data and never touch the network.

mod law;
mod markdown;
mod national;
mod results;

use anyhow::{Result, anyhow};
use scraper::{ElementRef, Html, Selector};

pub(crate) use law::{parse_law_page, parse_law_text, parse_law_text_link};
pub(crate) use national::{parse_agencies, parse_years};
pub(crate) use results::parse_results;

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|error| anyhow!("invalid selector `{css}`: {error}"))
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text of `element` with whitespace collapsed, or `None` when it is blank.
fn element_text(element: ElementRef) -> Option<String> {
    Some(collapse_whitespace(&element.text().collect::<String>())).filter(|text| !text.is_empty())
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
    fn element_text_collapses_whitespace_and_skips_blank_elements() {
        // Arrange
        let document = Html::parse_fragment("<p> Ley \n  1 </p><p>  </p>");
        let paragraph_selector = Selector::parse("p").expect("valid selector");
        let texts: Vec<Option<String>> = document
            .select(&paragraph_selector)
            .map(element_text)
            .collect();

        // Assert
        assert_eq!(texts, vec![Some("Ley 1".to_owned()), None]);
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
}
