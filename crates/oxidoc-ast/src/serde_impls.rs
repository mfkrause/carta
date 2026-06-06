//! Hand-written serde codecs for the records whose JSON form is a positional array rather than a
//! tagged object, plus the top-level [`Document`] map.
//!
//! Each array record (de)serializes through a Rust tuple: a tuple of references serializes as a
//! JSON array, and a JSON array deserializes into a tuple, so these impls stay short and obviously
//! correct without bespoke visitors. [`Document`] is the one exception — it is a fixed three-key
//! object whose first key is the [`crate::API_VERSION_KEY`] constant, so it gets a small map
//! serializer and a `MapAccess` visitor.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ast::{
    Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, TableBody, TableFoot, TableHead,
    Target, Text,
};

impl Serialize for Attr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.id, &self.classes, &self.attributes).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Attr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (id, classes, attributes) =
            <(Text, Vec<Text>, Vec<(Text, Text)>)>::deserialize(deserializer)?;
        Ok(Attr {
            id,
            classes,
            attributes,
        })
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.url, &self.title).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (url, title) = <(Text, Text)>::deserialize(deserializer)?;
        Ok(Target { url, title })
    }
}

impl Serialize for ListAttributes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.start, &self.style, &self.delim).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ListAttributes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (start, style, delim) =
            <(i32, ListNumberStyle, ListNumberDelim)>::deserialize(deserializer)?;
        Ok(ListAttributes {
            start,
            style,
            delim,
        })
    }
}

impl Serialize for Caption {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.short, &self.long).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Caption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (short, long) = <(Option<Vec<Inline>>, Vec<Block>)>::deserialize(deserializer)?;
        Ok(Caption { short, long })
    }
}

impl Serialize for ColSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.align, &self.width).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ColSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (align, width) = <(crate::ast::Alignment, ColWidth)>::deserialize(deserializer)?;
        Ok(ColSpec { align, width })
    }
}

impl Serialize for Table {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.attr,
            &self.caption,
            &self.col_specs,
            &self.head,
            &self.bodies,
            &self.foot,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Table {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, caption, col_specs, head, bodies, foot) = <(
            Attr,
            Caption,
            Vec<ColSpec>,
            TableHead,
            Vec<TableBody>,
            TableFoot,
        )>::deserialize(deserializer)?;
        Ok(Table {
            attr,
            caption,
            col_specs,
            head,
            bodies,
            foot,
        })
    }
}

impl Serialize for TableHead {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.attr, &self.rows).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TableHead {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, rows) = <(Attr, Vec<Row>)>::deserialize(deserializer)?;
        Ok(TableHead { attr, rows })
    }
}

impl Serialize for TableBody {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.attr, self.row_head_columns, &self.head, &self.body).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TableBody {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, row_head_columns, head, body) =
            <(Attr, i32, Vec<Row>, Vec<Row>)>::deserialize(deserializer)?;
        Ok(TableBody {
            attr,
            row_head_columns,
            head,
            body,
        })
    }
}

impl Serialize for TableFoot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.attr, &self.rows).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TableFoot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, rows) = <(Attr, Vec<Row>)>::deserialize(deserializer)?;
        Ok(TableFoot { attr, rows })
    }
}

impl Serialize for Row {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.attr, &self.cells).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Row {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, cells) = <(Attr, Vec<Cell>)>::deserialize(deserializer)?;
        Ok(Row { attr, cells })
    }
}

impl Serialize for Cell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.attr,
            &self.align,
            self.row_span,
            self.col_span,
            &self.content,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (attr, align, row_span, col_span, content) =
            <(Attr, crate::ast::Alignment, i32, i32, Vec<Block>)>::deserialize(deserializer)?;
        Ok(Cell {
            attr,
            align,
            row_span,
            col_span,
            content,
        })
    }
}

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
