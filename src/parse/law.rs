//! Parser of a single law page.

use std::path::Path;

use anyhow::{Result, bail};
use scraper::Html;

use super::{collapse_whitespace, selector};
use crate::model::{Jurisdiccion, LawDetails};
use crate::site::law_url;

const PROVINCE_SELECTOR: &str = ".label";

const LAW_TITLE_SELECTOR: &str = "h1.h5";

const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";

pub(crate) fn parse_law_page(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    pdf: &Path,
) -> Result<LawDetails> {
    let document = Html::parse_document(html);
    let text_of = |css: &str| -> Result<Option<String>> {
        Ok(document
            .select(&selector(css)?)
            .next()
            .map(|element| collapse_whitespace(&element.text().collect::<String>()))
            .filter(|text| !text.is_empty()))
    };
    let (Some(provincia), Some(titulo)) =
        (text_of(PROVINCE_SELECTOR)?, text_of(LAW_TITLE_SELECTOR)?)
    else {
        bail!("law not found: no province or title on the page; check `--law`");
    };
    Ok(LawDetails {
        provincia,
        jurisdiccion,
        titulo,
        ley: ley.to_owned(),
        estado: text_of(LAW_STATUS_SELECTOR)?,
        url: law_url(jurisdiccion, ley),
        pdf: pdf.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::site::SITE_ORIGIN;

    const LAW_FIXTURE: &str = include_str!("../../tests/fixtures/law.html");
    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn parse_law_page_buenos_aires_fixture_returns_province_title_status() -> Result<()> {
        // Act
        let details = parse_law_page(
            LAW_FIXTURE,
            Jurisdiccion::Provincial,
            LAW_SLUG,
            Path::new("out/ley.pdf"),
        )?;

        // Assert
        assert_eq!(
            details,
            LawDetails {
                provincia: "Buenos Aires".to_owned(),
                jurisdiccion: Jurisdiccion::Provincial,
                titulo: "Ley 14709".to_owned(),
                ley: LAW_SLUG.to_owned(),
                estado: Some("Vigente, de alcance general".to_owned()),
                url: format!("{SITE_ORIGIN}/normativa/provincial/{LAW_SLUG}"),
                pdf: "out/ley.pdf".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn parse_law_page_page_without_label_returns_not_found_error() {
        // Arrange
        let html = "<html><body><h1>Página no encontrada</h1></body></html>";

        // Act
        let result = parse_law_page(
            html,
            Jurisdiccion::Provincial,
            "ley-1",
            Path::new("ley-1.pdf"),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law not found")));
    }
}
