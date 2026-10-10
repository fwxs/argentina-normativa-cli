//! Domain types shared by the parsers and the commands.

use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct LawDetails {
    pub(crate) provincia: String,
    pub(crate) jurisdiccion: Jurisdiccion,
    pub(crate) titulo: String,
    pub(crate) ley: String,
    pub(crate) estado: Option<String>,
    pub(crate) url: String,
    pub(crate) pdf: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Normativa {
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

#[derive(Debug, PartialEq)]
pub(crate) struct ResultsPage {
    pub(crate) total_pages: usize,
    pub(crate) rows: Vec<Normativa>,
}
