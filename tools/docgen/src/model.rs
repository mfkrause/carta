//! The `docs/status.toml` schema, plus the shared vocabulary the renderers agree on.

use serde::{Deserialize, Serialize};

/// The whole of `docs/status.toml`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Status {
    pub meta: Meta,
    #[serde(default, rename = "format")]
    pub formats: Vec<Format>,
    pub extensions: Extensions,
    #[serde(default, rename = "feature")]
    pub features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Meta {
    pub oracle_version: String,
    pub ast_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Format {
    pub name: String,
    pub title: String,
    pub family: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub read: Support,
    pub write: Support,
    #[serde(default)]
    pub read_feature: Option<String>,
    #[serde(default)]
    pub write_feature: Option<String>,
    #[serde(default)]
    pub read_gaps: Vec<String>,
    #[serde(default)]
    pub write_gaps: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Extensions {
    pub supported: Vec<String>,
    pub recognized_not_modeled: Vec<String>,
    #[serde(default, rename = "gap")]
    pub gaps: Vec<ExtensionGap>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExtensionGap {
    pub names: Vec<String>,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Feature {
    pub name: String,
    pub status: Support,
    #[serde(default)]
    pub notes: String,
}

/// How far along one direction of one format is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    Usable,
    InDevelopment,
    NotStarted,
    NotApplicable,
}

impl Support {
    /// The glyph this status carries in the status tables.
    pub fn marker(self) -> &'static str {
        match self {
            Support::Usable => "✅",
            Support::InDevelopment => "🚧",
            Support::NotStarted => "❌",
            Support::NotApplicable => "➖",
        }
    }

    /// Whether the format ships in this direction, at any maturity.
    pub fn ships(self) -> bool {
        matches!(self, Support::Usable | Support::InDevelopment)
    }
}

/// Family keys in presentation order, paired with the heading each renders under.
pub const FAMILIES: &[(&str, &str)] = &[
    ("markdown", "Markdown family"),
    ("html", "HTML & slides"),
    ("tex", "TeX & typesetting"),
    ("lightweight", "Lightweight markup"),
    ("wiki", "Wikis"),
    ("roff", "roff"),
    ("office", "Word processor, ebook & notebook"),
    ("xml", "XML & publishing"),
    ("bibliography", "Bibliography"),
    ("data", "Data, interchange & terminal"),
];

/// One side of the conversion pipeline, so the renderers describe readers and writers once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

impl Direction {
    pub fn support(self, format: &Format) -> Support {
        match self {
            Direction::Read => format.read,
            Direction::Write => format.write,
        }
    }

    pub fn gaps(self, format: &Format) -> &[String] {
        match self {
            Direction::Read => &format.read_gaps,
            Direction::Write => &format.write_gaps,
        }
    }

    pub fn feature(self, format: &Format) -> Option<&str> {
        match self {
            Direction::Read => format.read_feature.as_deref(),
            Direction::Write => format.write_feature.as_deref(),
        }
    }

    /// Every format name this build accepts in this direction, canonical names and aliases alike.
    pub fn registry_names(self) -> Vec<&'static str> {
        match self {
            Direction::Read => carta::input_format_names(),
            Direction::Write => carta::output_format_names(),
        }
    }
}

impl Status {
    /// The formats in presentation order: family order first, then name.
    pub fn sorted_formats(&self) -> Vec<&Format> {
        let mut formats: Vec<&Format> = self.formats.iter().collect();
        formats.sort_by(|a, b| {
            family_rank(&a.family)
                .cmp(&family_rank(&b.family))
                .then_with(|| a.name.cmp(&b.name))
        });
        formats
    }
}

/// A family's position in [`FAMILIES`], or the end for a key that is not declared there.
pub fn family_rank(family: &str) -> usize {
    FAMILIES
        .iter()
        .position(|(key, _)| *key == family)
        .unwrap_or(FAMILIES.len())
}
