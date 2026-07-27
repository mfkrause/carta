//! Renders the JSON the documentation site's components read.

use serde::Serialize;

use crate::model::{Direction, FAMILIES, Format, Status, Support};

#[derive(Serialize)]
struct Formats<'a> {
    families: Vec<Family<'a>>,
    formats: Vec<FormatEntry<'a>>,
}

#[derive(Serialize)]
struct Family<'a> {
    key: &'a str,
    title: &'a str,
}

#[derive(Serialize)]
struct FormatEntry<'a> {
    name: &'a str,
    title: &'a str,
    family: &'a str,
    aliases: &'a [String],
    read: DirectionEntry<'a>,
    write: DirectionEntry<'a>,
}

#[derive(Serialize)]
struct DirectionEntry<'a> {
    status: Support,
    ships: bool,
    feature: Option<&'a str>,
    gaps: &'a [String],
}

#[derive(Serialize)]
struct ExtensionsFile<'a> {
    count: usize,
    supported: &'a [String],
    #[serde(rename = "recognizedNotModeled")]
    recognized_not_modeled: &'a [String],
    gaps: &'a [crate::model::ExtensionGap],
}

/// `formats.json`: every format with its per-direction status, feature flag, and gap list.
pub fn formats_json(status: &Status) -> Result<String, serde_json::Error> {
    let file = Formats {
        families: FAMILIES
            .iter()
            .map(|(key, title)| Family { key, title })
            .collect(),
        formats: status
            .sorted_formats()
            .into_iter()
            .map(|format| FormatEntry {
                name: &format.name,
                title: &format.title,
                family: &format.family,
                aliases: &format.aliases,
                read: direction_entry(format, Direction::Read),
                write: direction_entry(format, Direction::Write),
            })
            .collect(),
    };
    pretty(&file)
}

/// `extensions.json`: the supported and recognized sets plus the tracked per-extension gaps.
pub fn extensions_json(status: &Status) -> Result<String, serde_json::Error> {
    pretty(&ExtensionsFile {
        count: carta::Extension::COUNT,
        supported: &status.extensions.supported,
        recognized_not_modeled: &status.extensions.recognized_not_modeled,
        gaps: &status.extensions.gaps,
    })
}

fn direction_entry(format: &Format, direction: Direction) -> DirectionEntry<'_> {
    let support = direction.support(format);
    DirectionEntry {
        status: support,
        ships: support.ships(),
        feature: direction.feature(format),
        gaps: direction.gaps(format),
    }
}

fn pretty<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(value)?;
    json.push('\n');
    Ok(json)
}
