//! Cross-checks the format registry against `docs/status.toml`, so a format cannot be added,
//! renamed, or removed without the status data (and everything generated from it) following.
//!
//! The check is deliberately asymmetric: the status data is a superset of the registry, because it
//! also tracks formats that are not started yet. So every registry name must be documented as
//! shipping, and every name documented as shipping must be in the registry, while `not-started` and
//! `not-applicable` entries are unconstrained roadmap rows.
//!
//! Gated on `full` because the registry enumerators report only the formats a build compiled in; a
//! single-direction build would otherwise read as a pile of deleted formats.

#![cfg(feature = "full")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use serde::Deserialize;

#[derive(Deserialize)]
struct Status {
    #[serde(rename = "format")]
    formats: Vec<Format>,
}

#[derive(Deserialize)]
struct Format {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    read: Support,
    write: Support,
}

#[derive(Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum Support {
    Usable,
    InDevelopment,
    NotStarted,
    NotApplicable,
}

impl Support {
    fn ships(self) -> bool {
        matches!(self, Support::Usable | Support::InDevelopment)
    }
}

#[derive(Clone, Copy)]
struct Direction {
    label: &'static str,
    field: &'static str,
    support: fn(&Format) -> Support,
}

const READ: Direction = Direction {
    label: "reader",
    field: "read",
    support: |format| format.read,
};
const WRITE: Direction = Direction {
    label: "writer",
    field: "write",
    support: |format| format.write,
};

fn status() -> Status {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/status.toml");
    let source = std::fs::read_to_string(path).expect("docs/status.toml is readable");
    toml::from_str(&source).expect("docs/status.toml parses")
}

/// Every name the status data claims ships in `direction`, canonical names and aliases alike.
fn documented(status: &Status, direction: Direction) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    for format in &status.formats {
        if (direction.support)(format).ships() {
            names.insert(format.name.as_str());
            names.extend(format.aliases.iter().map(String::as_str));
        }
    }
    names
}

fn assert_registry_documented(registry: &[&str], direction: Direction) {
    let status = status();
    let documented = documented(&status, direction);
    for name in registry {
        assert!(
            documented.contains(name),
            "format {name:?} is in the {} registry but docs/status.toml does not mark \
             {} as usable or in-development; update docs/status.toml",
            direction.label,
            direction.field,
        );
    }
}

fn assert_documented_in_registry(registry: &[&str], direction: Direction) {
    let status = status();
    for format in &status.formats {
        if !(direction.support)(format).ships() {
            continue;
        }
        assert!(
            registry.contains(&format.name.as_str()),
            "docs/status.toml marks {:?} as {} = shipping, but it is not in the {} registry; \
             set {} to \"not-started\" or \"not-applicable\", or restore the format",
            format.name,
            direction.field,
            direction.label,
            direction.field,
        );
    }
}

#[test]
fn every_reader_in_the_registry_is_documented() {
    assert_registry_documented(&carta::input_format_names(), READ);
}

#[test]
fn every_writer_in_the_registry_is_documented() {
    assert_registry_documented(&carta::output_format_names(), WRITE);
}

#[test]
fn every_documented_reader_is_in_the_registry() {
    assert_documented_in_registry(&carta::input_format_names(), READ);
}

#[test]
fn every_documented_writer_is_in_the_registry() {
    assert_documented_in_registry(&carta::output_format_names(), WRITE);
}

#[test]
fn roadmap_entries_are_unconstrained() {
    let status = status();
    let readers = carta::input_format_names();
    let writers = carta::output_format_names();
    let roadmap: Vec<&Format> = status
        .formats
        .iter()
        .filter(|format| !format.read.ships() && !format.write.ships())
        .collect();

    assert!(
        !roadmap.is_empty(),
        "docs/status.toml tracks no not-started formats; the roadmap rows were lost"
    );
    for format in roadmap {
        assert!(
            !readers.contains(&format.name.as_str()) && !writers.contains(&format.name.as_str()),
            "format {:?} ships in the registry but docs/status.toml tracks it as roadmap only",
            format.name
        );
    }
}
