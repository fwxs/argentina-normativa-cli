//! Domain types shared by the parsers and the commands.

use clap::ValueEnum;
use serde::Serialize;

/// What `fetch` prints: the jurisdiction and, under `metadata`, the fields that jurisdiction has.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "jurisdiccion", content = "metadata", rename_all = "lowercase")]
pub(crate) enum LawDetails {
    Provincial(ProvincialMetadata),
    Nacional(NationalMetadata),
}

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct ProvincialMetadata {
    pub(crate) provincia: String,
    pub(crate) titulo: String,
    pub(crate) ley: String,
    pub(crate) estado: Option<String>,
    pub(crate) url: String,
    // File the law text was saved to.
    pub(crate) archivo: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct NationalMetadata {
    pub(crate) titulo: String,
    // Issuing agency.
    pub(crate) organismo: Option<String>,
    pub(crate) ley: String,
    pub(crate) url: String,
    // File the law text was saved to.
    pub(crate) archivo: String,
}

/// What a law page gives: the details to print and where its law text page is.
#[derive(Debug, PartialEq)]
pub(crate) struct LawPage {
    pub(crate) details: LawDetails,
    // Site path of the law text page to fetch next.
    pub(crate) text_path: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Normativa {
    // Only provincial rows carry a province.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provincia: Option<String>,
    pub(crate) jurisdiccion: Jurisdiccion,
    pub(crate) tipo_norma: String,
    pub(crate) titulo: String,
    // Issuing agency; only national rows carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) organismo: Option<String>,
    // Last path segment of the law url, e.g. "ley-11035-123456789-0abc-defg-373-0000svorpyel".
    pub(crate) ley: String,
    pub(crate) url: String,
    pub(crate) fecha_publicacion: Option<String>,
    pub(crate) descripcion: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Jurisdiccion {
    Provincial,
    Nacional,
}

impl Jurisdiccion {
    pub(crate) fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "provincial" => Some(Self::Provincial),
            "nacional" => Some(Self::Nacional),
            _ => None,
        }
    }

    pub(crate) fn path_segment(self) -> &'static str {
        match self {
            Self::Provincial => "provincial",
            Self::Nacional => "nacional",
        }
    }
}

/// The versions of a law text the site serves, each on its own page below the law page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LawTextKind {
    /// Current text, with every later amendment applied.
    Updated,
    /// Text as first published.
    Original,
}

impl LawTextKind {
    pub(crate) fn path_suffix(self) -> &'static str {
        match self {
            Self::Updated => "/actualizacion",
            Self::Original => "/texto",
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct ResultsPage {
    pub(crate) total_pages: usize,
    pub(crate) rows: Vec<Normativa>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn law_details_provincial_serializes_metadata_under_jurisdiction() -> serde_json::Result<()> {
        // Arrange
        let details = LawDetails::Provincial(ProvincialMetadata {
            provincia: "Mendoza".to_owned(),
            titulo: "Ley 9733".to_owned(),
            ley: "ley-9733".to_owned(),
            estado: Some("Vigente, de alcance general".to_owned()),
            url: "https://www.argentina.gob.ar/normativa/provincial/ley-9733".to_owned(),
            archivo: "ley-9733.md".to_owned(),
        });

        // Act
        let json = serde_json::to_string(&details)?;

        // Assert
        assert_eq!(
            json,
            r#"{"jurisdiccion":"provincial","metadata":{"provincia":"Mendoza","titulo":"Ley 9733","ley":"ley-9733","estado":"Vigente, de alcance general","url":"https://www.argentina.gob.ar/normativa/provincial/ley-9733","archivo":"ley-9733.md"}}"#
        );
        Ok(())
    }

    #[test]
    fn law_details_nacional_serializes_agency_without_province_or_status() -> serde_json::Result<()>
    {
        // Arrange
        let details = LawDetails::Nacional(NationalMetadata {
            titulo: "Decreto 1160/2026".to_owned(),
            organismo: Some("PODER EJECUTIVO NACIONAL (P.E.N.)".to_owned()),
            ley: "norma-431079".to_owned(),
            url: "https://www.argentina.gob.ar/normativa/nacional/norma-431079".to_owned(),
            archivo: "norma-431079.md".to_owned(),
        });

        // Act
        let json = serde_json::to_string(&details)?;

        // Assert
        assert_eq!(
            json,
            r#"{"jurisdiccion":"nacional","metadata":{"titulo":"Decreto 1160/2026","organismo":"PODER EJECUTIVO NACIONAL (P.E.N.)","ley":"norma-431079","url":"https://www.argentina.gob.ar/normativa/nacional/norma-431079","archivo":"norma-431079.md"}}"#
        );
        Ok(())
    }
}
