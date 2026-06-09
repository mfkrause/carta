//! Hand-written serde codecs for the records whose JSON form is a positional array rather than a
//! tagged object, plus the top-level [`Document`] map.
//!
//! Each array record (de)serializes through a Rust tuple: a tuple of references serializes as a
//! JSON array, and a JSON array deserializes into a tuple. [`array_record`] generates both
//! directions from one ordered field list, so the wire order is declared once and cannot drift
//! between serialize and deserialize. [`Document`] is the exception — it is a fixed three-key
//! object whose first key is the [`crate::API_VERSION_KEY`] constant (kept out of
//! `#[serde(rename)]` so the literal stays confined to that constant), so it keeps a small map
//! serializer and a `MapAccess` visitor.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, TableBody, TableFoot, TableHead,
    Target, Text,
};

/// Generates `Serialize`/`Deserialize` for a struct whose JSON representation is a positional
/// array, from a single ordered `field: Type` list. Declaring the order once removes the hazard of
/// hand-written impls silently disagreeing — easy to miss where adjacent fields share a type, as in
/// a cell's `row_span`/`col_span` or a table body's `head`/`body`.
macro_rules! array_record {
    ($name:ident { $($field:ident : $ty:ty),+ $(,)? }) => {
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                ($(&self.$field,)+).serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let ($($field,)+) = <($($ty,)+)>::deserialize(deserializer)?;
                Ok($name { $($field,)+ })
            }
        }
    };
}

array_record!(Attr {
    id: Text,
    classes: Vec<Text>,
    attributes: Vec<(Text, Text)>,
});
array_record!(Target {
    url: Text,
    title: Text,
});
array_record!(ListAttributes {
    start: i32,
    style: ListNumberStyle,
    delim: ListNumberDelim,
});
array_record!(Caption {
    short: Option<Vec<Inline>>,
    long: Vec<Block>,
});
array_record!(ColSpec {
    align: Alignment,
    width: ColWidth,
});
array_record!(Table {
    attr: Attr,
    caption: Caption,
    col_specs: Vec<ColSpec>,
    head: TableHead,
    bodies: Vec<TableBody>,
    foot: TableFoot,
});
array_record!(TableHead {
    attr: Attr,
    rows: Vec<Row>,
});
array_record!(TableBody {
    attr: Attr,
    row_head_columns: i32,
    head: Vec<Row>,
    body: Vec<Row>,
});
array_record!(TableFoot {
    attr: Attr,
    rows: Vec<Row>,
});
array_record!(Row {
    attr: Attr,
    cells: Vec<Cell>,
});
array_record!(Cell {
    attr: Attr,
    align: Alignment,
    row_span: i32,
    col_span: i32,
    content: Vec<Block>,
});

const DOCUMENT_FIELDS: &[&str] = &[crate::API_VERSION_KEY, "meta", "blocks"];

impl Serialize for Document {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry(crate::API_VERSION_KEY, &self.api_version)?;
        map.serialize_entry("meta", &self.meta)?;
        map.serialize_entry("blocks", &self.blocks)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(DocumentVisitor)
    }
}

struct DocumentVisitor;

impl<'de> Visitor<'de> for DocumentVisitor {
    type Value = Document;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a document object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
        let mut api_version = None;
        let mut meta = None;
        let mut blocks = None;

        while let Some(key) = map.next_key::<String>()? {
            if key == crate::API_VERSION_KEY {
                if api_version.is_some() {
                    return Err(de::Error::duplicate_field(crate::API_VERSION_KEY));
                }
                api_version = Some(map.next_value()?);
            } else if key == "meta" {
                if meta.is_some() {
                    return Err(de::Error::duplicate_field("meta"));
                }
                meta = Some(map.next_value::<BTreeMap<Text, MetaValue>>()?);
            } else if key == "blocks" {
                if blocks.is_some() {
                    return Err(de::Error::duplicate_field("blocks"));
                }
                blocks = Some(map.next_value::<Vec<Block>>()?);
            } else {
                return Err(de::Error::unknown_field(&key, DOCUMENT_FIELDS));
            }
        }

        Ok(Document {
            api_version: api_version
                .ok_or_else(|| de::Error::missing_field(crate::API_VERSION_KEY))?,
            meta: meta.unwrap_or_default(),
            blocks: blocks.ok_or_else(|| de::Error::missing_field("blocks"))?,
        })
    }
}
