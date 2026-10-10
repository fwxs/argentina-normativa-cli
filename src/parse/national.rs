//! Parsers of the national search form's select boxes.

use anyhow::{Context, Result};

use super::parse_options;

// The national page repeats some selects in a second form; only the main search form counts.
const NATIONAL_FORM_SELECTOR: &str = "form#infoleg-normativa-search-form";

pub(crate) fn parse_agencies(html: &str) -> Result<Vec<String>> {
    parse_options(
        html,
        &format!("{NATIONAL_FORM_SELECTOR} select[name=dependencia]"),
    )
}

pub(crate) fn parse_years(html: &str) -> Result<Vec<u16>> {
    parse_options(html, &format!("{NATIONAL_FORM_SELECTOR} select[name=anio]"))?
        .iter()
        .map(|year| {
            year.parse()
                .with_context(|| format!("year option `{year}` is not a number"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_agencies_national_fixture_ignores_placeholder_and_second_form() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let agencies = parse_agencies(&html)?;

        // Assert
        assert_eq!(
            agencies,
            vec![
                "1RA. COM. NAC. DE SALARIOS IND. DEL VESTIDO TRABAJO A DOMIC.".to_owned(),
                "ADM. GRAL. DEL SERVICIO NACIONAL DE SANIDAD ANIMAL".to_owned(),
                "ADMINISTRACION ADUANA BARILOCHE".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn parse_years_national_fixture_keeps_site_order_as_numbers() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let years = parse_years(&html)?;

        // Assert
        assert_eq!(years, vec![2026, 2025, 2024]);
        Ok(())
    }

    #[test]
    fn parse_years_non_numeric_option_returns_error() {
        // Arrange
        let html = r#"<form id="infoleg-normativa-search-form"><select name="anio">
            <option value="dos mil">x</option></select></form>"#;

        // Act
        let result = parse_years(html);

        // Assert
        assert!(result.is_err());
    }
}
