//! Parsers of a single law page and of its text page.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use scraper::Html;

use super::markdown::{escape_inline, render_blocks};
use super::{element_text, selector};
use crate::model::{Jurisdiccion, LawDetails, LawPage};
use crate::site::{law_text_path, law_url};

const PROVINCE_SELECTOR: &str = ".label";

// Issuing agency of a national norm, above its subtitle (`p.lead` without `m-b-0`).
const AGENCY_SELECTOR: &str = "p.lead.m-b-0";

const LAW_TITLE_SELECTOR: &str = "h1.h5";

const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";

const PROVINCIAL_LAW_TEXT_SELECTOR: &str = "article.m-t-2";

// Holds the original and the updated text of a national norm alike.
const NATIONAL_LAW_TEXT_SELECTOR: &str = "div.infoleg-norma-body";

const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";

fn law_text_selector(jurisdiccion: Jurisdiccion) -> &'static str {
    match jurisdiccion {
        Jurisdiccion::Provincial => PROVINCIAL_LAW_TEXT_SELECTOR,
        Jurisdiccion::Nacional => NATIONAL_LAW_TEXT_SELECTOR,
    }
}

/// The details of a law page and the path of its law text page. The page is parsed once for both.
pub(crate) fn parse_law_page(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    archivo: &Path,
) -> Result<LawPage> {
    let document = Html::parse_document(html);
    Ok(LawPage {
        details: parse_law_details(&document, jurisdiccion, ley, archivo)?,
        text_path: parse_law_text_link(&document, jurisdiccion, ley)?,
    })
}

fn parse_law_details(
    document: &Html,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    archivo: &Path,
) -> Result<LawDetails> {
    let text_of = |css: &str| -> Result<Option<String>> {
        Ok(document
            .select(&selector(css)?)
            .next()
            .and_then(element_text))
    };
    let Some(titulo) = text_of(LAW_TITLE_SELECTOR)? else {
        bail!("law not found: no title on the page; check `--law`");
    };
    let (provincia, organismo, estado) = match jurisdiccion {
        Jurisdiccion::Provincial => {
            let Some(provincia) = text_of(PROVINCE_SELECTOR)? else {
                bail!("law not found: no province on the page; check `--law`");
            };
            (Some(provincia), None, text_of(LAW_STATUS_SELECTOR)?)
        }
        // National pages have neither a province nor a status.
        Jurisdiccion::Nacional => (None, text_of(AGENCY_SELECTOR)?, None),
    };
    Ok(LawDetails {
        provincia,
        jurisdiccion,
        titulo,
        organismo,
        ley: ley.to_owned(),
        estado,
        url: law_url(jurisdiccion, ley),
        archivo: archivo.display().to_string(),
    })
}

/// Site path of the law text page, from the law page's "Ver norma" links. The updated text is
/// preferred over the original one when the page offers both. A link is only followed when it is
/// exactly the law text path of the requested law, so the page can't point the next request
/// anywhere else (and a text page the page doesn't link to, which would be a 404, is never asked for).
fn parse_law_text_link(document: &Html, jurisdiccion: Jurisdiccion, ley: &str) -> Result<String> {
    let hrefs: Vec<&str> = document
        .select(&selector(VIEW_LAW_BUTTON_SELECTOR)?)
        .filter_map(|link| link.value().attr("href"))
        .collect();
    if hrefs.is_empty() {
        bail!("no \"Ver norma\" link on the law page");
    }
    let expected: Vec<String> = jurisdiccion
        .text_kinds()
        .iter()
        .map(|&kind| law_text_path(jurisdiccion, ley, kind))
        .collect();
    expected
        .iter()
        .find(|path| hrefs.contains(&path.as_str()))
        .cloned()
        .ok_or_else(|| {
            // The page's own hrefs are untrusted and unbounded, so only their count is reported.
            anyhow!(
                "no \"Ver norma\" link matches the expected paths {expected:?} ({} found)",
                hrefs.len()
            )
        })
}

/// A law text page (`<law url>/actualizacion` or `/texto`) as Markdown: the title as a heading, then one
/// Markdown block per block of the law article.
pub(crate) fn parse_law_text(html: &str, jurisdiccion: Jurisdiccion) -> Result<String> {
    let document = Html::parse_document(html);
    let text_selector = law_text_selector(jurisdiccion);
    let Some(article) = document.select(&selector(text_selector)?).next() else {
        bail!("law text not found: no `{text_selector}` on the page");
    };
    let title = document
        .select(&selector(LAW_TITLE_SELECTOR)?)
        .next()
        .and_then(element_text);
    // The article opens with a screen-reader-only "Norma" heading that is not part of the text.
    let markdown: Vec<String> = title
        .map(|title| format!("# {}", escape_inline(&title)))
        .into_iter()
        .chain(render_blocks(article))
        .collect();
    Ok(format!("{}\n", markdown.join("\n\n")))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::site::SITE_ORIGIN;

    const LAW_FIXTURE: &str = include_str!("../../tests/fixtures/law.html");
    const LAW_TEXT_FIXTURE: &str = include_str!("../../tests/fixtures/law_text.html");
    const LAW_TEXT_BLOCKS_FIXTURE: &str = include_str!("../../tests/fixtures/law_text_blocks.html");
    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    fn text_link(html: &str, jurisdiccion: Jurisdiccion, ley: &str) -> Result<String> {
        parse_law_text_link(&Html::parse_document(html), jurisdiccion, ley)
    }

    #[test]
    fn parse_law_page_buenos_aires_fixture_returns_province_title_status() -> Result<()> {
        // Act
        let page = parse_law_page(
            LAW_FIXTURE,
            Jurisdiccion::Provincial,
            LAW_SLUG,
            Path::new("out/ley.md"),
        )?;

        // Assert
        assert_eq!(
            page.details,
            LawDetails {
                provincia: Some("Buenos Aires".to_owned()),
                jurisdiccion: Jurisdiccion::Provincial,
                titulo: "Ley 14709".to_owned(),
                organismo: None,
                ley: LAW_SLUG.to_owned(),
                estado: Some("Vigente, de alcance general".to_owned()),
                url: format!("{SITE_ORIGIN}/normativa/provincial/{LAW_SLUG}"),
                archivo: "out/ley.md".to_owned(),
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
            Path::new("ley-1.md"),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law not found")));
    }

    #[test]
    fn parse_law_text_mendoza_fixture_renders_title_and_articles() -> Result<()> {
        // Act
        let markdown = parse_law_text(LAW_TEXT_FIXTURE, Jurisdiccion::Provincial)?;

        // Assert
        let paragraphs: Vec<&str> = markdown.trim_end().split("\n\n").collect();
        assert_eq!(paragraphs[0], "# Ley 9689");
        assert_eq!(
            paragraphs[1],
            "Declaran el año 2026 como Año del 90 Aniversario de la Fiesta Nacional de la Vendimia"
        );
        assert!(
            paragraphs
                .iter()
                .any(|paragraph| paragraph.starts_with("Artículo 1°- Declárase el año 2026"))
        );
        assert!(paragraphs.contains(&"**Firmantes**"));
        assert!(!paragraphs.contains(&"Norma"));
        assert!(markdown.ends_with("LETTRY\n"));
        Ok(())
    }

    #[test]
    fn parse_law_text_blocks_fixture_renders_lists_tables_and_escapes() -> Result<()> {
        // Act
        let markdown = parse_law_text(LAW_TEXT_BLOCKS_FIXTURE, Jurisdiccion::Provincial)?;

        // Assert
        let expected = [
            "# Ley 1",
            "Considerando:",
            "- Primero\n- Segundo\n    - Sub uno",
            "1. Uno\n2. Dos",
            "| Rubro | Monto |\n| --- | --- |\n| Tasa \\| A | 10 |",
            "**Doble** negrita y *cursiva*",
            "\\# no es título",
            "\\- no es lista",
            "1\\. no es lista numerada",
            "Usa \\*asterisco\\* y snake\\_case y \\[corchetes\\]",
            "> Cita",
            "### Capítulo II",
            "Texto suelto **negrita**",
        ];
        assert_eq!(markdown, format!("{}\n", expected.join("\n\n")));
        Ok(())
    }

    #[test]
    fn parse_law_text_inline_bold_keeps_surrounding_text() -> Result<()> {
        // Arrange
        let html = r#"<h1 class="h5">Ley 1</h1><article class="m-t-2">
            <p>Art. 1 <strong>Derógase</strong> la ley 2.</p><p>  </p></article>"#;

        // Act
        let markdown = parse_law_text(html, Jurisdiccion::Provincial)?;

        // Assert
        assert_eq!(markdown, "# Ley 1\n\nArt. 1 **Derógase** la ley 2.\n");
        Ok(())
    }

    #[test]
    fn parse_law_text_page_without_article_returns_error() {
        // Arrange
        let html = "<html><body><h1 class=\"h5\">Ley 1</h1></body></html>";

        // Act
        let result = parse_law_text(html, Jurisdiccion::Provincial);

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law text not found")));
    }

    #[test]
    fn parse_law_text_link_law_fixture_returns_actualizacion_href() -> Result<()> {
        // Act
        let href = text_link(LAW_FIXTURE, Jurisdiccion::Provincial, LAW_SLUG)?;

        // Assert
        assert_eq!(
            href,
            format!("/normativa/provincial/{LAW_SLUG}/actualizacion")
        );
        Ok(())
    }

    #[test]
    fn parse_law_text_link_unexpected_href_returns_error() {
        // Act / Assert
        for href in [
            "https://example.com/normativa/provincial/ley-1/actualizacion",
            "/normativa/provincial/ley-1",
            "/normativa/municipal/ley-1/actualizacion",
            "/normativa/provincial/a/b/actualizacion",
            "/normativa/provincial/ley-1?next=x/actualizacion",
            "/normativa/provincial/ley-1#frag/actualizacion",
            "/normativa/provincial/../actualizacion",
            "/normativa/nacional/ley-1/actualizacion",
            "/normativa/provincial/ley-2/actualizacion",
            "/normativa/provincial/ley-1/texto",
        ] {
            let html = format!(r#"<a class="btn btn-primary" href="{href}">Ver norma</a>"#);
            let result = text_link(&html, Jurisdiccion::Provincial, "ley-1");
            assert!(result.is_err(), "accepted `{href}`");
        }
    }

    #[test]
    fn parse_law_text_link_mismatch_error_does_not_echo_page_hrefs() {
        // Arrange
        let links = (0..500)
            .map(|index| format!(r#"<a class="btn btn-primary" href="/evil-{index}">x</a>"#))
            .collect::<String>();

        // Act
        let result = text_link(&links, Jurisdiccion::Nacional, "norma-1");

        // Assert
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(message.contains("/normativa/nacional/norma-1/texto"));
        assert!(message.contains("500 found"));
        assert!(!message.contains("evil"));
        assert!(message.len() < 300, "{} bytes", message.len());
    }

    #[test]
    fn parse_law_text_link_page_without_button_returns_error() {
        // Act
        let result = text_link(
            "<html><body></body></html>",
            Jurisdiccion::Provincial,
            "ley-1",
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("no \"Ver norma\" link")));
    }

    const NATIONAL_LAW_FIXTURE: &str = include_str!("../../tests/fixtures/national_law.html");
    const NATIONAL_LAW_TEXT_FIXTURE: &str =
        include_str!("../../tests/fixtures/national_law_text.html");
    const NATIONAL_LAW_UPDATED_FIXTURE: &str =
        include_str!("../../tests/fixtures/national_law_updated.html");
    const NATIONAL_LAW_TEXT_UPDATED_FIXTURE: &str =
        include_str!("../../tests/fixtures/national_law_text_updated.html");
    const NATIONAL_SLUG: &str = "norma-431079";

    #[test]
    fn parse_law_page_national_fixture_returns_title_and_agency_without_province() -> Result<()> {
        // Act
        let page = parse_law_page(
            NATIONAL_LAW_FIXTURE,
            Jurisdiccion::Nacional,
            NATIONAL_SLUG,
            Path::new("norma-431079.md"),
        )?;

        // Assert
        assert_eq!(
            page.details,
            LawDetails {
                provincia: None,
                jurisdiccion: Jurisdiccion::Nacional,
                titulo: "Decreto 1160/2026".to_owned(),
                organismo: Some("PODER EJECUTIVO NACIONAL (P.E.N.)".to_owned()),
                ley: NATIONAL_SLUG.to_owned(),
                estado: None,
                url: format!("{SITE_ORIGIN}/normativa/nacional/{NATIONAL_SLUG}"),
                archivo: "norma-431079.md".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn parse_law_page_national_page_without_title_returns_not_found_error() {
        // Arrange
        let html = "<html><body><h1>Página no encontrada</h1></body></html>";

        // Act
        let result = parse_law_page(
            html,
            Jurisdiccion::Nacional,
            "norma-1",
            Path::new("norma-1.md"),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law not found")));
    }

    #[test]
    fn parse_law_text_link_national_original_only_returns_texto_href() -> Result<()> {
        // Act
        let href = text_link(NATIONAL_LAW_FIXTURE, Jurisdiccion::Nacional, NATIONAL_SLUG)?;

        // Assert
        assert_eq!(href, "/normativa/nacional/norma-431079/texto");
        Ok(())
    }

    #[test]
    fn parse_law_text_link_national_with_updated_text_prefers_actualizacion() -> Result<()> {
        // Act
        let href = text_link(
            NATIONAL_LAW_UPDATED_FIXTURE,
            Jurisdiccion::Nacional,
            "norma-638",
        )?;

        // Assert
        assert_eq!(href, "/normativa/nacional/norma-638/actualizacion");
        Ok(())
    }

    #[test]
    fn parse_law_text_link_national_foreign_hrefs_return_error() {
        // Act / Assert
        for href in [
            "https://example.com/normativa/nacional/norma-1/texto",
            "/normativa/nacional/norma-2/texto",
            "/normativa/nacional/norma-1/../texto",
            "/normativa/nacional/norma-1/texto?next=x",
            "/normativa/provincial/norma-1/texto",
        ] {
            let html =
                format!(r#"<a class="btn btn-primary" href="{href}">Ver norma original</a>"#);
            let result = text_link(&html, Jurisdiccion::Nacional, "norma-1");
            assert!(result.is_err(), "accepted `{href}`");
        }
    }

    #[test]
    fn parse_law_text_national_original_fixture_renders_title_and_paragraphs() -> Result<()> {
        // Act
        let markdown = parse_law_text(NATIONAL_LAW_TEXT_FIXTURE, Jurisdiccion::Nacional)?;

        // Assert
        let paragraphs: Vec<&str> = markdown.trim_end().split("\n\n").collect();
        assert_eq!(paragraphs[0], "# Decreto 1160/2026");
        assert!(paragraphs.contains(&"Ciudad de Buenos Aires, 08/10/2026"));
        assert!(
            paragraphs
                .iter()
                .any(|paragraph| paragraph.starts_with("En uso de las facultades"))
        );
        assert!(markdown.ends_with("e. 09/10/2026 N° 75183/26 v. 09/10/2026\n"));
        assert!(!markdown.contains('\u{E000}'));
        Ok(())
    }

    #[test]
    fn parse_law_text_national_updated_fixture_renders_title_and_articles() -> Result<()> {
        // Act
        let markdown = parse_law_text(NATIONAL_LAW_TEXT_UPDATED_FIXTURE, Jurisdiccion::Nacional)?;

        // Assert
        assert!(markdown.starts_with("# Ley 24240\n\n"));
        assert!(markdown.contains("ARTICULO 1"));
        assert!(markdown.matches("\n\n").count() > 50);
        Ok(())
    }
}
