//! Renders the JSON the documentation site's components read.

use std::collections::BTreeMap;

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
    #[serde(rename = "byFormat")]
    by_format: BTreeMap<&'a str, FormatExtensions>,
}

/// The extension names one format accepts, and which of them its defaults turn on.
#[derive(Serialize)]
struct FormatExtensions {
    accepted: Vec<&'static str>,
    enabled: Vec<&'static str>,
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

/// `extensions.json`: the supported and recognized sets, the tracked per-extension gaps, and the
/// set each shipping format accepts.
pub fn extensions_json(status: &Status) -> Result<String, serde_json::Error> {
    pretty(&ExtensionsFile {
        count: carta::Extension::COUNT,
        supported: &status.extensions.supported,
        recognized_not_modeled: &status.extensions.recognized_not_modeled,
        gaps: &status.extensions.gaps,
        by_format: by_format(status),
    })
}

/// The accepted extension set per shipping format, straight from the library's own resolver.
fn by_format(status: &Status) -> BTreeMap<&str, FormatExtensions> {
    let mut sets = BTreeMap::new();
    for format in &status.formats {
        if !format.read.ships() && !format.write.ships() {
            continue;
        }
        let Ok(entries) = carta::format_extensions(Some(&format.name)) else {
            continue;
        };
        sets.insert(
            format.name.as_str(),
            FormatExtensions {
                accepted: entries.iter().map(|(name, _)| name.name()).collect(),
                enabled: entries
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(name, _)| name.name())
                    .collect(),
            },
        );
    }
    sets
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
