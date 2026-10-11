//! Parsers of a single law page and of its text page.

use std::path::Path;

use anyhow::{Result, bail};
use scraper::{ElementRef, Html, Node};

use super::{collapse_whitespace, selector};
use crate::model::{Jurisdiccion, LawDetails};
use crate::site::{law_text_path, law_url};

const PROVINCE_SELECTOR: &str = ".label";

const LAW_TITLE_SELECTOR: &str = "h1.h5";

const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";

const LAW_TEXT_SELECTOR: &str = "article.m-t-2";

const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";

pub(crate) fn parse_law_page(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    archivo: &Path,
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
        archivo: archivo.display().to_string(),
    })
}

/// Site path of the law text page, from the law page's "Ver norma" link. The link must be exactly
/// the law text path of the requested law, so the page can't point the next request anywhere else.
pub(crate) fn parse_law_text_link(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
) -> Result<String> {
    let document = Html::parse_document(html);
    let Some(href) = document
        .select(&selector(VIEW_LAW_BUTTON_SELECTOR)?)
        .find_map(|link| link.value().attr("href"))
    else {
        bail!("no \"Ver norma\" link on the law page");
    };
    let expected = law_text_path(jurisdiccion, ley);
    if href != expected {
        bail!("unexpected \"Ver norma\" link `{href}`, expected `{expected}`");
    }
    Ok(expected)
}

/// Inline text of `element` with `<strong>`/`<b>` rendered as Markdown bold.
fn inline_markdown(element: ElementRef) -> String {
    element
        .children()
        .map(|node| match node.value() {
            Node::Text(text) => text.to_string(),
            Node::Element(child) => {
                let inner = ElementRef::wrap(node)
                    .map(inline_markdown)
                    .unwrap_or_default();
                match child.name() {
                    // Bold markers must hug the text, so move the element's own edge spaces outside.
                    "strong" | "b" if !inner.trim().is_empty() => {
                        let leading = if inner.starts_with(char::is_whitespace) {
                            " "
                        } else {
                            ""
                        };
                        let trailing = if inner.ends_with(char::is_whitespace) {
                            " "
                        } else {
                            ""
                        };
                        format!("{leading}**{}**{trailing}", inner.trim())
                    }
                    "br" => " ".to_owned(),
                    _ => inner,
                }
            }
            _ => String::new(),
        })
        .collect()
}

/// The law text page (`<law url>/actualizacion`) as Markdown: the title as a heading, then one
/// paragraph per block of the law article.
pub(crate) fn parse_law_text(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let Some(article) = document.select(&selector(LAW_TEXT_SELECTOR)?).next() else {
        bail!("law text not found: no `{LAW_TEXT_SELECTOR}` on the page");
    };
    let title = document
        .select(&selector(LAW_TITLE_SELECTOR)?)
        .next()
        .map(|element| collapse_whitespace(&element.text().collect::<String>()))
        .filter(|text| !text.is_empty());
    // The article opens with a screen-reader-only "Norma" heading that is not part of the text.
    let paragraphs = article
        .child_elements()
        .filter(|block| !block.value().classes().any(|class| class == "sr-only"))
        .map(|block| collapse_whitespace(&inline_markdown(block)))
        .filter(|paragraph| !paragraph.is_empty());
    let markdown: Vec<String> = title
        .map(|title| format!("# {title}"))
        .into_iter()
        .chain(paragraphs)
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
    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn parse_law_page_buenos_aires_fixture_returns_province_title_status() -> Result<()> {
        // Act
        let details = parse_law_page(
            LAW_FIXTURE,
            Jurisdiccion::Provincial,
            LAW_SLUG,
            Path::new("out/ley.md"),
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
        let markdown = parse_law_text(LAW_TEXT_FIXTURE)?;

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
    fn parse_law_text_inline_bold_keeps_surrounding_text() -> Result<()> {
        // Arrange
        let html = r#"<h1 class="h5">Ley 1</h1><article class="m-t-2">
            <p>Art. 1 <strong>Derógase</strong> la ley 2.</p><p>  </p></article>"#;

        // Act
        let markdown = parse_law_text(html)?;

        // Assert
        assert_eq!(markdown, "# Ley 1\n\nArt. 1 **Derógase** la ley 2.\n");
        Ok(())
    }

    #[test]
    fn parse_law_text_page_without_article_returns_error() {
        // Arrange
        let html = "<html><body><h1 class=\"h5\">Ley 1</h1></body></html>";

        // Act
        let result = parse_law_text(html);

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law text not found")));
    }

    #[test]
    fn parse_law_text_link_law_fixture_returns_actualizacion_href() -> Result<()> {
        // Act
        let href = parse_law_text_link(LAW_FIXTURE, Jurisdiccion::Provincial, LAW_SLUG)?;

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
        ] {
            let html = format!(r#"<a class="btn btn-primary" href="{href}">Ver norma</a>"#);
            let result = parse_law_text_link(&html, Jurisdiccion::Provincial, "ley-1");
            assert!(result.is_err(), "accepted `{href}`");
        }
    }

    #[test]
    fn parse_law_text_link_page_without_button_returns_error() {
        // Act
        let result = parse_law_text_link(
            "<html><body></body></html>",
            Jurisdiccion::Provincial,
            "ley-1",
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("no \"Ver norma\" link")));
    }
}
