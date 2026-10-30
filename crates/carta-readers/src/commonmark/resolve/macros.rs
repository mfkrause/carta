//! Expand embedded TeX in source order while keeping code and note scopes separate.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use carta_ast::{Block, Document, Inline};
use carta_core::Extensions;

use super::{HeaderNumbering, HeaderParseCache, RefContext, cite_budget_for, resolve_blocks};
use crate::commonmark::{ExampleMap, FootnoteDefs, IrBlock, RefMap};
use crate::latex::MacroExpander;

pub(in crate::commonmark) const SOURCE: &str = "\0tex-source";
pub(in crate::commonmark) const NOTE: &str = "\0tex-note";

type Engine = Rc<RefCell<MacroExpander>>;

pub(in crate::commonmark) struct Context<'a> {
    engine: Engine,
    definitions: &'a FootnoteDefs,
    notes: Rc<RefCell<BTreeMap<String, Vec<Block>>>>,
    active: Rc<Cell<bool>>,
}

impl<'a> Context<'a> {
    pub(in crate::commonmark) fn for_source(
        source: &str,
        extensions: Extensions,
        definitions: &'a FootnoteDefs,
    ) -> Option<Self> {
        (extensions.contains(carta_core::Extension::LatexMacros) && source.contains('\\')).then(
            || {
                Self::new(
                    Rc::new(RefCell::new(MacroExpander::new(source.len(), extensions))),
                    definitions,
                )
            },
        )
    }

    fn new(engine: Engine, definitions: &'a FootnoteDefs) -> Self {
        Self {
            engine,
            definitions,
            notes: Rc::new(RefCell::new(BTreeMap::new())),
            active: Rc::new(Cell::new(false)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::commonmark) fn blocks(
        &self,
        ir: &[IrBlock],
        refs: &RefMap,
        footnotes: &FootnoteDefs,
        examples: &ExampleMap,
        ext: Extensions,
        markdown: bool,
        source_len: usize,
    ) -> Option<Vec<Block>> {
        let blocks = resolve(
            ir,
            refs,
            footnotes,
            examples,
            ext,
            markdown,
            source_len,
            Some(Rc::clone(&self.engine)),
        );
        if blocks.is_some() {
            self.active.set(true);
        }
        blocks
    }

    fn fork(&self) -> Self {
        Self {
            engine: Rc::new(RefCell::new(self.engine.borrow().fork())),
            definitions: self.definitions,
            notes: Rc::clone(&self.notes),
            active: Rc::clone(&self.active),
        }
    }

    pub(super) fn raw(&self, source: &str) -> String {
        let mut engine = self.engine.borrow_mut();
        let output = engine.raw(source);
        if engine.has_definitions() || output != source {
            self.active.set(true);
        }
        output
    }

    pub(in crate::commonmark) fn inlines(&self, inlines: &mut [Inline]) {
        carta_core::with_stack(|| {
            for inline in inlines {
                match inline {
                    Inline::Math(_, source) => {
                        let output = self.engine.borrow().math(source);
                        if output != source.as_str() {
                            self.active.set(true);
                        }
                        *source = output.into();
                    }
                    Inline::RawInline(format, source) if format.0 == SOURCE => {
                        *source = self.raw(source).into();
                        format.0 = "tex".into();
                    }
                    Inline::Emph(children)
                    | Inline::Underline(children)
                    | Inline::Strong(children)
                    | Inline::Strikeout(children)
                    | Inline::Superscript(children)
                    | Inline::Subscript(children)
                    | Inline::SmallCaps(children)
                    | Inline::Quoted(_, children)
                    | Inline::Span(_, children)
                    | Inline::Link(_, children, _)
                    | Inline::Image(_, children, _) => self.inlines(children),
                    Inline::Cite(citations, fallback) => {
                        self.fork().inlines(fallback);
                        for citation in citations {
                            self.inlines(&mut citation.prefix);
                            self.inlines(&mut citation.suffix);
                        }
                    }
                    Inline::Note(blocks) => {
                        for block in blocks {
                            if let Block::Plain(inlines) | Block::Para(inlines) = block {
                                self.inlines(inlines);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    pub(super) fn definition(
        &self,
        label: &str,
        refs: &RefMap,
        notes: RefContext,
        ext: Extensions,
    ) {
        if self.notes.borrow().contains_key(label) {
            return;
        }
        let Some(body) = self.definitions.get(label) else {
            return;
        };
        self.notes.borrow_mut().insert(label.to_owned(), Vec::new());
        let scope = self.fork();
        let context = RefContext {
            macros: Some(&scope),
            in_definition: true,
            ..notes
        };
        let blocks = resolve_blocks(body, refs, context, ext, &mut HeaderParseCache::new());
        self.notes.borrow_mut().insert(label.to_owned(), blocks);
    }

    fn fill_notes(&self, blocks: Vec<Block>) -> Vec<Block> {
        let mut document = Document {
            blocks,
            ..Document::default()
        };
        carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
            if let Inline::Note(blocks) = inline
                && let [Block::RawBlock(format, label)] = blocks.as_slice()
                && format.0 == NOTE
            {
                let body = self
                    .notes
                    .borrow()
                    .get(label.as_str())
                    .cloned()
                    .unwrap_or_default();
                *blocks = body;
            }
        });
        document.blocks
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve(
    ir: &[IrBlock],
    refs: &RefMap,
    footnotes: &FootnoteDefs,
    examples: &ExampleMap,
    ext: Extensions,
    markdown: bool,
    source_len: usize,
    inherited: Option<Engine>,
) -> Option<Vec<Block>> {
    let engine =
        inherited.unwrap_or_else(|| Rc::new(RefCell::new(MacroExpander::new(source_len, ext))));
    // Keep the starting bindings shared so both passes charge the same scope copies.
    let initial = engine.borrow().fork();
    // Heading targets need expanded text before the final link pass.
    let scratch = Context::new(Rc::new(RefCell::new(initial.probe())), footnotes);
    let defined: BTreeSet<String> = footnotes.keys().cloned().collect();
    let empty = BTreeMap::new();
    let count = Cell::new(0);
    let budget = cite_budget_for(source_len);
    let probe = RefContext {
        defined: &defined,
        by_id: &empty,
        macros: Some(&scratch),
        directory: None,
        in_definition: false,
        markdown,
        examples,
        cite_count: &count,
        cite_budget: &budget,
    };
    let blocks = resolve_blocks(ir, refs, probe, ext, &mut HeaderParseCache::new());
    if !scratch.active.get() {
        return None;
    }
    let mut expanded = VecDeque::new();
    collect_headers(&blocks, &mut expanded);
    let context = Context::new(engine, footnotes);
    let mut refs = refs.clone();
    let mut cache = HeaderParseCache::new();
    let probe = RefContext {
        macros: Some(&context),
        ..probe
    };
    if ext.contains(carta_core::Extension::ImplicitHeaderReferences) {
        super::gather_expanded_headers(
            ir,
            &mut refs,
            probe,
            ext,
            &mut HeaderNumbering::new(ext, markdown),
            &mut cache,
            &mut expanded,
        );
    }
    count.set(0);
    budget.set(cite_budget_for(source_len).get());
    let blocks = resolve_blocks(ir, &refs, probe, ext, &mut cache);
    let mut blocks = context.fill_notes(blocks);
    crate::commonmark::identifiers::assign_header_identifiers(&mut blocks, ext, markdown);
    Some(blocks)
}

fn collect_headers(blocks: &[Block], headers: &mut VecDeque<Vec<Inline>>) {
    carta_core::with_stack(|| {
        for block in blocks {
            match block {
                Block::Header(_, _, inlines) => headers.push_back(inlines.clone()),
                Block::Div(_, children) | Block::BlockQuote(children) => {
                    collect_headers(children, headers);
                }
                Block::OrderedList(_, items) | Block::BulletList(items) => {
                    for item in items {
                        collect_headers(item, headers);
                    }
                }
                Block::DefinitionList(items) => {
                    for (_, definitions) in items {
                        for definition in definitions {
                            collect_headers(definition, headers);
                        }
                    }
                }
                _ => {}
            }
        }
    });
}
