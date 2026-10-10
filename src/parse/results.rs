//! Parser of the search results pages (provincial and national).

use anyhow::Result;
use scraper::Html;

use super::{collapse_whitespace, selector};
use crate::model::{Jurisdiccion, Normativa, ResultsPage};
use crate::site::{SITE_ORIGIN, TIPO_NORMA, parse_law_href};

/// Pages in the results counter: provincial "<total> normas encontradas en <pages> páginas" has the
/// page count in its second `span`; national has it only as text ("... en 25 páginas").
fn parse_total_pages(document: &Html) -> Result<usize> {
    let provincial_counter = selector("div.m-b-2 span.fw-bold")?;
    let national_counter = selector("div.infoleg-search-results-count")?;

    let national_pages = document
        .select(&national_counter)
        .next()
        .and_then(|counter| {
            let text = collapse_whitespace(&counter.text().collect::<String>());
            let words: Vec<&str> = text.split(' ').collect();
            words
                .windows(3)
                .find(|window| window[0] == "en" && window[2].starts_with("página"))
                .and_then(|window| window[1].parse().ok())
        });
    let provincial_pages = || {
        document
            .select(&provincial_counter)
            .nth(1)
            .and_then(|span| {
                collapse_whitespace(&span.text().collect::<String>())
                    .parse()
                    .ok()
            })
    };
    // Absent counter means nothing matched.
    Ok(national_pages.or_else(provincial_pages).unwrap_or(0))
}

/// "Resolución GENERAL 5911/2026" -> "Resolución GENERAL": the words before the number.
fn norm_type_from_title(titulo: &str) -> String {
    let words: Vec<&str> = titulo
        .split_whitespace()
        .take_while(|word| !word.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    if words.is_empty() {
        titulo.to_owned()
    } else {
        words.join(" ")
    }
}

pub(crate) fn parse_results(html: &str, provincia: Option<&str>) -> Result<ResultsPage> {
    let document = Html::parse_document(html);
    // Provincial rows sit in `#normas`; national ones in the div right after the results counter.
    let row_selector = selector("#normas tbody tr, .infoleg-search-results-count ~ div tbody tr")?;
    let link_selector = selector(r#"td[data-label="Normativa"] a"#)?;
    let agency_selector = selector(r#"td[data-label="Normativa"] p.small"#)?;
    let time_selector = selector("time[datetime]")?;
    let description_selector = selector(r#"td[data-label="Descripción"] p.small"#)?;

    let total_pages = parse_total_pages(&document)?;

    let rows = document
        .select(&row_selector)
        .filter_map(|row| {
            let link = row.select(&link_selector).next()?;
            let href = link.value().attr("href")?;
            let Some((jurisdiccion, ley)) = parse_law_href(href) else {
                tracing::warn!(href, "unrecognised normativa link");
                return None;
            };
            let titulo = collapse_whitespace(&link.text().collect::<String>());
            Some(Normativa {
                provincia: provincia.map(str::to_owned),
                jurisdiccion,
                tipo_norma: match jurisdiccion {
                    Jurisdiccion::Provincial => TIPO_NORMA.to_owned(),
                    Jurisdiccion::Nacional => norm_type_from_title(&titulo),
                },
                titulo,
                organismo: row
                    .select(&agency_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .find(|text| !text.is_empty()),
                ley: ley.to_owned(),
                url: format!("{SITE_ORIGIN}{href}"),
                fecha_publicacion: row
                    .select(&time_selector)
                    .next()
                    .and_then(|time| time.value().attr("datetime"))
                    .map(str::to_owned),
                descripcion: row
                    .select(&description_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .filter(|text| !text.is_empty())
                    .collect(),
            })
        })
        .collect();

    Ok(ResultsPage { total_pages, rows })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const RESULTS_FIXTURE: &str = include_str!("../../tests/fixtures/results.html");
    const NATIONAL_RESULTS_FIXTURE: &str =
        include_str!("../../tests/fixtures/results_national.html");

    #[test]
    fn parse_results_buenos_aires_laws_page_returns_fifty_rows_over_three_pages() -> Result<()> {
        // Arrange / Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.total_pages, 3);
        assert_eq!(results.rows.len(), 50);
        let first = &results.rows[0];
        assert_eq!(first.provincia.as_deref(), Some("Buenos Aires"));
        assert_eq!(first.jurisdiccion, Jurisdiccion::Provincial);
        assert_eq!(first.tipo_norma, "Ley");
        assert!(first.ley.starts_with("ley-"));
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/provincial/{}", first.ley)
        );
        assert!(!first.titulo.is_empty());
        assert!(first.fecha_publicacion.is_some());
        Ok(())
    }

    #[test]
    fn parse_results_no_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = "<html><body><p>No se encontraron normas</p></body></html>";

        // Act
        let results = parse_results(html, Some("Chaco"))?;

        // Assert
        assert_eq!(
            results,
            ResultsPage {
                total_pages: 0,
                rows: vec![]
            }
        );
        Ok(())
    }

    #[test]
    fn parse_results_without_province_serialises_null_province() -> Result<()> {
        // Arrange
        let html = r#"<div id="normas"><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/ley-1-abc"> Ley 1 </a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;
        let json = serde_json::to_string(&results.rows[0])?;

        // Assert
        assert!(json.contains(r#""provincia":null"#));
        assert!(json.contains(r#""jurisdiccion":"nacional""#));
        assert!(json.contains(r#""ley":"ley-1-abc""#));
        Ok(())
    }

    #[test]
    fn parse_results_national_fixture_returns_five_rows_over_212_pages() -> Result<()> {
        // Act
        let results = parse_results(NATIONAL_RESULTS_FIXTURE, None)?;

        // Assert
        assert_eq!(results.total_pages, 212);
        assert_eq!(results.rows.len(), 5);
        let first = &results.rows[0];
        assert_eq!(first.provincia, None);
        assert_eq!(first.jurisdiccion, Jurisdiccion::Nacional);
        assert_eq!(first.tipo_norma, "Ley");
        assert_eq!(first.titulo, "Ley 27826");
        assert_eq!(first.ley, "norma-431078");
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/nacional/norma-431078")
        );
        assert_eq!(
            first.organismo.as_deref(),
            Some("HONORABLE CONGRESO DE LA NACION ARGENTINA")
        );
        assert_eq!(first.fecha_publicacion.as_deref(), Some("2026-10-09"));
        assert_eq!(first.descripcion.len(), 3);
        assert_eq!(results.rows[1].tipo_norma, "Resolución GENERAL");
        Ok(())
    }

    #[test]
    fn parse_results_national_zero_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">0</span> norma encontrada</div>
            <table><tbody><tr><td colspan="3">No se encontraron resultados</td></tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(
            results,
            ResultsPage {
                total_pages: 0,
                rows: vec![]
            }
        );
        Ok(())
    }

    #[test]
    fn parse_results_provincial_fixture_rows_are_all_provincial_without_organismo() -> Result<()> {
        // Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.rows.len(), 50);
        for row in &results.rows {
            assert_eq!(row.jurisdiccion, Jurisdiccion::Provincial);
            assert_eq!(row.tipo_norma, "Ley");
            assert!(!serde_json::to_string(row)?.contains("organismo"));
        }
        Ok(())
    }

    #[test]
    fn parse_results_ignores_rows_of_unrelated_tables() -> Result<()> {
        // Arrange
        let html = r#"<table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.rows, vec![]);
        Ok(())
    }

    #[test]
    fn parse_results_national_single_result_page_counts_one_page() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">1</span> norma encontrada en 1 página</div>
            <div class=""><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.total_pages, 1);
        assert_eq!(results.rows.len(), 1);
        Ok(())
    }

    #[test]
    fn norm_type_from_title_drops_number_and_year() {
        // Act / Assert
        assert_eq!(
            norm_type_from_title("Resolución GENERAL 5911/2026"),
            "Resolución GENERAL"
        );
        assert_eq!(
            norm_type_from_title("Decisión Administrativa 3/2024"),
            "Decisión Administrativa"
        );
        assert_eq!(norm_type_from_title("27826"), "27826");
    }
}
