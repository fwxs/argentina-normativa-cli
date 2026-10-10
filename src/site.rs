//! Site constants and the url shapes of argentina.gob.ar/normativa.

use std::time::Duration;

use anyhow::{Context, Result};
use url::Url;

use crate::NationalFilters;
use crate::model::Jurisdiccion;

pub(crate) const SITE_ORIGIN: &str = "https://www.argentina.gob.ar";
pub(crate) const SEARCH_PATH: &str = "/normativa";
pub(crate) const PAGE_SIZE: &str = "50";
// The site's provincial search only accepts this norm type (the form field is disabled with it).
pub(crate) const TIPO_NORMA: &str = "Ley";

// `tipo_norma` slugs of the national search form; an empty value searches every type.
pub(crate) const LAW_TYPES: &[&str] = &[
    "leyes",
    "decretos",
    "decisiones_administrativas",
    "resoluciones",
    "disposiciones",
    "acordadas",
    "actas",
    "actuaciones",
    "acuerdos",
    "circulares",
    "comunicaciones",
    "comunicados",
    "convenios",
    "decisiones",
    "decretos_ley",
    "directivas",
    "instrucciones",
    "interpretacion",
    "laudos",
    "memorandums",
    "misiones",
    "notas",
    "notas_externas",
    "protocolos",
    "providencias",
    "recomendaciones",
];

// Default for `query national --max-pages`: a lone broad filter (`--law-type decretos`) is thousands of
// rows, and every page costs a crawl delay.
pub(crate) const DEFAULT_MAX_PAGES: u32 = 20;

// robots.txt asks for `Crawl-delay: 10`.
pub(crate) const CRAWL_DELAY: Duration = Duration::from_secs(10);

/// Splits a law link `/normativa/<jurisdiccion>/<ley>` into its jurisdiction and law slug.
pub(crate) fn parse_law_href(href: &str) -> Option<(Jurisdiccion, &str)> {
    let (segment, ley) = href
        .strip_prefix(SEARCH_PATH)?
        .strip_prefix('/')?
        .split_once('/')?;
    if ley.is_empty() || ley.contains('/') {
        return None;
    }
    Some((Jurisdiccion::from_path_segment(segment)?, ley))
}

pub(crate) fn law_url(jurisdiccion: Jurisdiccion, ley: &str) -> String {
    format!(
        "{SITE_ORIGIN}{SEARCH_PATH}/{}/{ley}",
        jurisdiccion.path_segment()
    )
}

pub(crate) fn search_url(provincia: &str, query: &str, page_number: usize) -> Result<String> {
    let page_number = page_number.to_string();
    let url = Url::parse_with_params(
        &format!("{SITE_ORIGIN}{SEARCH_PATH}"),
        [
            ("provincia", provincia),
            ("jurisdiccion", "provincial"),
            ("tipo_norma", TIPO_NORMA),
            ("texto", query),
            ("limit", PAGE_SIZE),
            // The site's `offset` is a 1-based page number.
            ("offset", page_number.as_str()),
        ],
    )
    .context("failed to build search url")?;
    Ok(url.into())
}

/// National search url; the site wants every field present (empty when unused) and a 0-based `page`.
pub(crate) fn national_search_url(filters: &NationalFilters, page_index: usize) -> Result<String> {
    let law_number = filters.law_number.map(|number| number.to_string());
    let year = filters.year.map(|year| year.to_string());
    let page_index = page_index.to_string();
    let url = Url::parse_with_params(
        &format!("{SITE_ORIGIN}{SEARCH_PATH}"),
        [
            ("jurisdiccion", "nacional"),
            (
                "tipo_norma",
                filters.law_type.as_deref().unwrap_or_default(),
            ),
            ("numero", law_number.as_deref().unwrap_or_default()),
            ("anio", year.as_deref().unwrap_or_default()),
            ("dependencia", filters.agency.as_deref().unwrap_or_default()),
            (
                "publicacion_desde",
                filters.from_date.as_deref().unwrap_or_default(),
            ),
            (
                "publicacion_hasta",
                filters.to_date.as_deref().unwrap_or_default(),
            ),
            ("texto", filters.query.as_deref().unwrap_or_default()),
            ("s", "1"),
            ("page", page_index.as_str()),
        ],
    )
    .context("failed to build national search url")?;
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_law_href_provincial_link_returns_jurisdiction_and_slug() {
        // Act
        let parsed =
            parse_law_href("/normativa/provincial/ley-11035-123456789-0abc-defg-373-0000svorpyel");

        // Assert
        assert_eq!(
            parsed,
            Some((
                Jurisdiccion::Provincial,
                "ley-11035-123456789-0abc-defg-373-0000svorpyel"
            ))
        );
    }

    #[test]
    fn parse_law_href_nacional_link_returns_nacional() {
        // Act
        let parsed = parse_law_href("/normativa/nacional/ley-27078-abc");

        // Assert
        assert_eq!(parsed, Some((Jurisdiccion::Nacional, "ley-27078-abc")));
    }

    #[test]
    fn parse_law_href_unknown_jurisdiction_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/municipal/ley-1"), None);
    }

    #[test]
    fn parse_law_href_missing_slug_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/provincial/"), None);
        assert_eq!(parse_law_href("/normativa/provincial"), None);
        assert_eq!(parse_law_href("/otra/provincial/ley-1"), None);
    }

    #[test]
    fn search_url_accented_province_is_percent_encoded() -> Result<()> {
        // Act
        let url = search_url("Río Negro", "impuesto tasa", 2)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?provincia=R%C3%ADo+Negro&jurisdiccion=provincial\
             &tipo_norma=Ley&texto=impuesto+tasa&limit=50&offset=2"
        );
        Ok(())
    }

    #[test]
    fn national_search_url_full_filters_are_encoded_in_site_order() -> Result<()> {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("decretos".to_owned()),
            law_number: Some(70),
            year: Some(2023),
            agency: Some("MINISTERIO DE ECONOMIA".to_owned()),
            from_date: Some("2023-01-01".to_owned()),
            to_date: Some("2023-12-31".to_owned()),
            query: Some("impuesto tasa".to_owned()),
        };

        // Act
        let url = national_search_url(&filters, 2)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?jurisdiccion=nacional&tipo_norma=decretos\
             &numero=70&anio=2023&dependencia=MINISTERIO+DE+ECONOMIA\
             &publicacion_desde=2023-01-01&publicacion_hasta=2023-12-31\
             &texto=impuesto+tasa&s=1&page=2"
        );
        Ok(())
    }

    #[test]
    fn national_search_url_only_query_keeps_empty_fields() -> Result<()> {
        // Arrange
        let filters = NationalFilters {
            query: Some("impuesto".to_owned()),
            ..Default::default()
        };

        // Act
        let url = national_search_url(&filters, 0)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?jurisdiccion=nacional&tipo_norma=&numero=\
             &anio=&dependencia=&publicacion_desde=&publicacion_hasta=&texto=impuesto&s=1&page=0"
        );
        Ok(())
    }
}
