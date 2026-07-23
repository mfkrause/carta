//! The `word/numbering.xml` part and the plan that drives it.
//!
//! Every document carries two scaffold definitions: a no-glyph bulleted definition (`990`) whose
//! concrete instance `1000` numbers the continuation paragraphs of multi-paragraph list items, and,
//! when a bulleted list appears, the visible bullet definition (`991`). Each ordered list
//! contributes a definition keyed by its marker style, delimiter and start number, so lists that
//! share those settings share one definition. Every list *instance* in the body binds to a definition
//! through its own concrete number, allocated in document order from `1001` upward.

use carta_ast::{ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::container::xml::Element;

/// Word-processing markup namespace, the only one this part declares.
const WML_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The no-glyph scaffold definition, always present.
const SCAFFOLD_ABSTRACT: u32 = 990;
/// The visible bullet definition, present when a bulleted list appears.
const BULLET_ABSTRACT: u32 = 991;
/// The unchecked task-list checkbox definition, present when an unchecked task item appears.
const UNCHECKED_ABSTRACT: u32 = 992;
/// The checked task-list checkbox definition, present when a checked task item appears.
const CHECKED_ABSTRACT: u32 = 993;
/// The concrete number bound to the scaffold definition; it numbers list-item continuation lines.
const SCAFFOLD_NUM: u32 = 1000;
/// The first concrete number available to a body list instance.
const FIRST_NUM: u32 = 1001;

/// Indentation step, in twips, between successive list levels.
const INDENT_STEP: u32 = 720;
/// The hanging indent, in twips, applied to every level's marker.
const HANGING: &str = "360";

/// The shape of an abstract list definition: the visible bullet cycle, or an ordered numbering.
#[derive(Clone, Copy)]
enum Shape {
    Bullet,
    /// A task-list checkbox bullet whose static glyph is the empty or ticked ballot box.
    Checkbox {
        checked: bool,
    },
    Ordered {
        start: i32,
        style: ListNumberStyle,
        delim: ListNumberDelim,
    },
}

/// The list definitions a document needs and the concrete numbers bound to them, built up as the
/// body is walked.
#[derive(Default)]
pub(super) struct ListPlan {
    /// Each concrete instance in document order: the abstract id it binds to and, for an ordered
    /// instance, the start value its number overrides to. Instance `i` is number `FIRST_NUM + i`.
    instances: Vec<(u32, Option<i32>)>,
    /// The distinct abstract definitions to emit, in first-appearance order (the scaffold aside).
    definitions: Vec<(u32, Shape)>,
}

impl ListPlan {
    /// Reserves a concrete number for one bulleted list instance and returns it.
    pub(super) fn bullet(&mut self) -> u32 {
        self.allocate(BULLET_ABSTRACT, Shape::Bullet)
    }

    /// Reserves a concrete number for one task-list item, bound to the checkbox definition its
    /// checked state selects. Each task item takes its own number, since adjacent items may differ in
    /// state and so bind to different definitions.
    pub(super) fn checkbox(&mut self, checked: bool) -> u32 {
        let id = if checked {
            CHECKED_ABSTRACT
        } else {
            UNCHECKED_ABSTRACT
        };
        self.allocate(id, Shape::Checkbox { checked })
    }

    /// Reserves a concrete number for one ordered list instance and returns it.
    pub(super) fn ordered(&mut self, attrs: &ListAttributes) -> u32 {
        let id = ordered_abstract_id(attrs);
        self.allocate(
            id,
            Shape::Ordered {
                start: attrs.start,
                style: attrs.style,
                delim: attrs.delim,
            },
        )
    }

    /// The concrete number that numbers list-item continuation paragraphs.
    #[allow(clippy::unused_self)] // Reads as a plan property; exposes the module-private scaffold number.
    pub(super) fn continuation_num(&self) -> u32 {
        SCAFFOLD_NUM
    }

    fn allocate(&mut self, abstract_id: u32, shape: Shape) -> u32 {
        if !self.definitions.iter().any(|(id, _)| *id == abstract_id) {
            self.definitions.push((abstract_id, shape));
        }
        let start_override = match shape {
            Shape::Ordered { start, .. } => Some(start),
            Shape::Bullet | Shape::Checkbox { .. } => None,
        };
        self.instances.push((abstract_id, start_override));
        FIRST_NUM + u32::try_from(self.instances.len().saturating_sub(1)).unwrap_or(0)
    }
}

/// The abstract identifier an ordered list maps to, encoding its marker style, delimiter and start so
/// lists that agree on all three share one definition.
fn ordered_abstract_id(attrs: &ListAttributes) -> u32 {
    let base = 99_000 + style_code(attrs.style) * 100 + delim_code(attrs.delim) * 10;
    let start = i64::from(attrs.start).clamp(0, 9);
    u32::try_from(i64::from(base) + start).unwrap_or(99_000)
}

/// The style component of an ordered definition's identifier.
fn style_code(style: ListNumberStyle) -> u32 {
    match style {
        ListNumberStyle::DefaultStyle => 2,
        ListNumberStyle::Example => 3,
        ListNumberStyle::Decimal => 4,
        ListNumberStyle::LowerRoman => 5,
        ListNumberStyle::UpperRoman => 6,
        ListNumberStyle::LowerAlpha => 7,
        ListNumberStyle::UpperAlpha => 8,
    }
}

/// The delimiter component of an ordered definition's identifier.
fn delim_code(delim: ListNumberDelim) -> u32 {
    match delim {
        ListNumberDelim::DefaultDelim => 0,
        ListNumberDelim::Period => 1,
        ListNumberDelim::OneParen => 2,
        ListNumberDelim::TwoParens => 3,
    }
}

/// The Word numbering format name for a marker style at a given nesting depth. A default-styled list
/// has no fixed format of its own, so it cycles by depth (decimal, lowercase letter, lowercase roman,
/// repeating) to keep nested levels visually distinct. Every other style keeps one format at every
/// level.
fn num_fmt(style: ListNumberStyle, depth: u32) -> &'static str {
    match style {
        ListNumberStyle::DefaultStyle => match depth % 3 {
            0 => "decimal",
            1 => "lowerLetter",
            _ => "lowerRoman",
        },
        ListNumberStyle::LowerRoman => "lowerRoman",
        ListNumberStyle::UpperRoman => "upperRoman",
        ListNumberStyle::LowerAlpha => "lowerLetter",
        ListNumberStyle::UpperAlpha => "upperLetter",
        _ => "decimal",
    }
}

/// The marker text for one ordered level: the level's placeholder wrapped by the delimiter.
fn level_text(delim: ListNumberDelim, level: u32) -> String {
    let placeholder = format!("%{}", level + 1);
    match delim {
        ListNumberDelim::OneParen => format!("{placeholder})"),
        ListNumberDelim::TwoParens => format!("({placeholder})"),
        _ => format!("{placeholder}."),
    }
}

/// The `nsid` value for an abstract definition: its identifier prefixed with `A`, padded to eight
/// characters.
fn nsid(abstract_id: u32) -> String {
    format!("{:0>8}", format!("A{abstract_id}"))
}

/// The indentation properties for a level at the given depth.
fn indent(depth: u32) -> Element {
    Element::new("w:pPr").child(
        Element::new("w:ind")
            .attr("w:left", &(INDENT_STEP * (depth + 1)).to_string())
            .attr("w:hanging", HANGING),
    )
}

/// One level of the no-glyph scaffold definition.
fn scaffold_level(depth: u32) -> Element {
    Element::new("w:lvl")
        .attr("w:ilvl", &depth.to_string())
        .child(Element::new("w:numFmt").attr("w:val", "bullet"))
        .child(Element::new("w:lvlText").attr("w:val", " "))
        .child(Element::new("w:lvlJc").attr("w:val", "left"))
        .child(indent(depth))
}

/// One level of the visible bullet definition: a glyph and font that cycle every three depths.
fn bullet_level(depth: u32) -> Element {
    // The glyph/font pairs cycle so nested bullets are visually distinct.
    let (glyph, font): (&str, &str) = match depth % 3 {
        0 => ("\u{f0b7}", "Symbol"),
        1 => ("o", "Courier New"),
        _ => ("\u{f0a7}", "Wingdings"),
    };
    Element::new("w:lvl")
        .attr("w:ilvl", &depth.to_string())
        .child(Element::new("w:numFmt").attr("w:val", "bullet"))
        .child(Element::new("w:lvlText").attr("w:val", glyph))
        .child(Element::new("w:lvlJc").attr("w:val", "left"))
        .child(indent(depth))
        .child(
            Element::new("w:rPr").child(
                Element::new("w:rFonts")
                    .attr("w:ascii", font)
                    .attr("w:hAnsi", font)
                    .attr("w:cs", font)
                    .attr("w:hint", "default"),
            ),
        )
}

/// One level of a task-list checkbox definition: a static ballot-box glyph, empty or ticked, used at
/// every depth. Unlike an ordinary bullet it carries no font override; the glyph stands on its own.
fn checkbox_level(checked: bool, depth: u32) -> Element {
    let glyph = if checked { "\u{2612}" } else { "\u{2610}" };
    Element::new("w:lvl")
        .attr("w:ilvl", &depth.to_string())
        .child(Element::new("w:numFmt").attr("w:val", "bullet"))
        .child(Element::new("w:lvlText").attr("w:val", glyph))
        .child(Element::new("w:lvlJc").attr("w:val", "left"))
        .child(indent(depth))
}

/// One level of an ordered definition: the marker style and text at that depth.
fn ordered_level(
    start: i32,
    style: ListNumberStyle,
    delim: ListNumberDelim,
    depth: u32,
) -> Element {
    Element::new("w:lvl")
        .attr("w:ilvl", &depth.to_string())
        .child(Element::new("w:start").attr("w:val", &start.to_string()))
        .child(Element::new("w:numFmt").attr("w:val", num_fmt(style, depth)))
        .child(Element::new("w:lvlText").attr("w:val", &level_text(delim, depth)))
        .child(Element::new("w:lvlJc").attr("w:val", "left"))
        .child(indent(depth))
}

/// Builds one abstract definition with all nine levels.
fn abstract_num(id: u32, shape: Shape) -> Element {
    let mut element = Element::new("w:abstractNum").attr("w:abstractNumId", &id.to_string());
    element.push(Element::new("w:nsid").attr("w:val", &nsid(id)));
    element.push(Element::new("w:multiLevelType").attr("w:val", "multilevel"));
    for depth in 0..9 {
        element.push(match shape {
            Shape::Bullet => bullet_level(depth),
            Shape::Checkbox { checked } => checkbox_level(checked, depth),
            Shape::Ordered {
                start,
                style,
                delim,
            } => ordered_level(start, style, delim, depth),
        });
    }
    element
}

/// The no-glyph scaffold definition with all nine levels.
fn scaffold_abstract() -> Element {
    let mut element =
        Element::new("w:abstractNum").attr("w:abstractNumId", &SCAFFOLD_ABSTRACT.to_string());
    element.push(Element::new("w:nsid").attr("w:val", &nsid(SCAFFOLD_ABSTRACT)));
    element.push(Element::new("w:multiLevelType").attr("w:val", "multilevel"));
    for depth in 0..9 {
        element.push(scaffold_level(depth));
    }
    element
}

/// A concrete number bound to an abstract definition. An ordered instance overrides its start value
/// at every level, so lists that share a definition still begin where each was declared to.
fn num(num_id: u32, abstract_id: u32, start_override: Option<i32>) -> Element {
    let mut element = Element::new("w:num")
        .attr("w:numId", &num_id.to_string())
        .child(Element::new("w:abstractNumId").attr("w:val", &abstract_id.to_string()));
    if let Some(start) = start_override {
        for depth in 0..9 {
            element.push(
                Element::new("w:lvlOverride")
                    .attr("w:ilvl", &depth.to_string())
                    .child(Element::new("w:startOverride").attr("w:val", &start.to_string())),
            );
        }
    }
    element
}

/// The complete `word/numbering.xml` part for a document's list plan.
pub(super) fn numbering_xml(plan: &ListPlan) -> String {
    let mut root = Element::new("w:numbering").attr("xmlns:w", WML_NS);
    root.push(scaffold_abstract());
    for (id, shape) in &plan.definitions {
        root.push(abstract_num(*id, *shape));
    }
    root.push(num(SCAFFOLD_NUM, SCAFFOLD_ABSTRACT, None));
    for (index, (abstract_id, start_override)) in plan.instances.iter().enumerate() {
        let num_id = FIRST_NUM + u32::try_from(index).unwrap_or(0);
        root.push(num(num_id, *abstract_id, *start_override));
    }
    root.render_document()
}
