//! Block accumulation for the docx reader, merging list, quote, code, and caption paragraphs.

use std::collections::{BTreeMap, VecDeque};

use carta_ast::{Block, Caption, Inline, ListAttributes};

/// One list paragraph awaiting reassembly into nested lists.
pub(super) struct ListEntry {
    pub(super) num_id: i32,
    pub(super) level: i32,
    pub(super) numbering: Option<ListAttributes>,
    pub(super) block: Block,
}

/// Whether the block most recently placed can still absorb a caption paragraph, and how.
#[derive(Default, PartialEq, Eq)]
enum Attachable {
    /// The last block is neither an image paragraph nor a table awaiting a caption.
    #[default]
    None,
    /// The last block is a lone-image paragraph that a caption turns into a figure.
    Figure,
    /// The last block is a table whose caption slot is still empty.
    Table,
}

/// Collects converted blocks while merging consecutive list, block-quote, and code paragraphs and
/// folding caption paragraphs into an adjacent image (as a figure) or table.
#[derive(Default)]
pub(super) struct BlockSink {
    blocks: Vec<Block>,
    pending_list: Vec<ListEntry>,
    pending_quote: Vec<Block>,
    pending_code: Vec<String>,
    /// A caption paragraph held for the block that follows it, set when no image or table precedes it.
    pending_caption: Option<Vec<Block>>,
    /// Whether the last placed block can still take a caption, so an image or table before a caption
    /// wins over one after it.
    last_attachable: Attachable,
    /// Running ordinal for each numbered level, keyed by `(numId, ilvl)`, so a list resumes its count
    /// after an interrupting block instead of restarting.
    list_counters: BTreeMap<(i32, i32), i32>,
}

impl BlockSink {
    /// Appends a finished block and records whether it can still take a caption.
    fn place(&mut self, block: Block, attachable: Attachable) {
        self.blocks.push(block);
        self.last_attachable = attachable;
    }

    /// Emits any held caption as its own paragraph, used when the block that follows it cannot be a
    /// figure or table target.
    fn release_caption(&mut self) {
        if let Some(long) = self.pending_caption.take() {
            for block in long {
                self.place(block, Attachable::None);
            }
        }
    }

    /// Records an ordinary finished block, first flushing merge runs and releasing any held caption.
    pub(super) fn emit(&mut self, block: Block) {
        self.flush();
        self.release_caption();
        self.place(block, Attachable::None);
    }

    /// Records a lone-image paragraph, forming a figure with a preceding caption when one is held.
    pub(super) fn emit_image(&mut self, inlines: Vec<Inline>) {
        self.flush();
        match self.pending_caption.take() {
            Some(long) => self.place(
                Block::Figure(
                    Box::default(),
                    Box::new(Caption { short: None, long }),
                    vec![Block::Plain(inlines)],
                ),
                Attachable::None,
            ),
            None => self.place(Block::Para(inlines), Attachable::Figure),
        }
    }

    /// Records a table, folding a preceding caption into its caption slot when one is held.
    pub(super) fn emit_table(&mut self, block: Block) {
        self.flush();
        if let Block::Table(mut table) = block {
            match self.pending_caption.take() {
                Some(long) => {
                    table.caption = Caption { short: None, long };
                    self.place(Block::Table(table), Attachable::None);
                }
                None => self.place(Block::Table(table), Attachable::Table),
            }
        } else {
            self.release_caption();
            self.place(block, Attachable::None);
        }
    }

    /// Records a caption paragraph, attaching it to a preceding image or table when one is available
    /// and otherwise holding it for the block that follows.
    pub(super) fn push_caption(&mut self, long: Vec<Block>) {
        self.flush();
        match self.last_attachable {
            Attachable::Figure if matches!(self.blocks.last(), Some(Block::Para(_))) => {
                self.last_attachable = Attachable::None;
                if let Some(Block::Para(inlines)) = self.blocks.pop() {
                    self.place(
                        Block::Figure(
                            Box::default(),
                            Box::new(Caption { short: None, long }),
                            vec![Block::Plain(inlines)],
                        ),
                        Attachable::None,
                    );
                }
            }
            Attachable::Table if matches!(self.blocks.last(), Some(Block::Table(_))) => {
                self.last_attachable = Attachable::None;
                if let Some(Block::Table(mut table)) = self.blocks.pop() {
                    table.caption = Caption { short: None, long };
                    self.place(Block::Table(table), Attachable::None);
                }
            }
            _ => {
                self.release_caption();
                self.pending_caption = Some(long);
            }
        }
    }

    /// Ends any merge run and caption context without placing a block, as a dropped empty paragraph
    /// does between an image and a caption.
    pub(super) fn interrupt(&mut self) {
        self.flush();
        self.release_caption();
        self.last_attachable = Attachable::None;
    }

    pub(super) fn push_list(&mut self, mut entry: ListEntry) {
        self.release_caption();
        self.flush_quote();
        self.flush_code();
        if let Some(attrs) = entry.numbering.as_mut() {
            let key = (entry.num_id, entry.level);
            let ordinal = match self.list_counters.get(&key) {
                Some(previous) => previous.saturating_add(1),
                None => attrs.start,
            };
            self.list_counters.insert(key, ordinal);
            // Advancing a level restarts every level nested under it.
            self.list_counters
                .retain(|(num_id, level), _| !(*num_id == entry.num_id && *level > entry.level));
            attrs.start = ordinal;
        }
        self.pending_list.push(entry);
    }

    pub(super) fn push_quote(&mut self, block: Block) {
        self.release_caption();
        self.flush_list();
        self.flush_code();
        self.pending_quote.push(block);
    }

    pub(super) fn push_code(&mut self, line: String) {
        self.release_caption();
        self.flush_list();
        self.flush_quote();
        self.pending_code.push(line);
    }

    pub(super) fn flush(&mut self) {
        self.flush_list();
        self.flush_quote();
        self.flush_code();
    }

    fn flush_list(&mut self) {
        if self.pending_list.is_empty() {
            return;
        }
        let entries = VecDeque::from(std::mem::take(&mut self.pending_list));
        for block in build_lists(entries) {
            self.place(block, Attachable::None);
        }
    }

    fn flush_quote(&mut self) {
        if self.pending_quote.is_empty() {
            return;
        }
        let inner = std::mem::take(&mut self.pending_quote);
        self.place(Block::BlockQuote(inner), Attachable::None);
    }

    fn flush_code(&mut self) {
        if self.pending_code.is_empty() {
            return;
        }
        let code = std::mem::take(&mut self.pending_code).join("\n");
        self.place(
            Block::CodeBlock(Box::default(), code.into()),
            Attachable::None,
        );
    }

    pub(super) fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.release_caption();
        self.blocks
    }
}

/// Reassembles list paragraphs into nested lists: a maximal span at the shallowest level forms one
/// list, a deeper span nests inside the preceding item, and a same-level paragraph that selects a
/// different numbering begins a fresh sibling list.
fn build_lists(mut entries: VecDeque<ListEntry>) -> Vec<Block> {
    let mut out = Vec::new();
    while !entries.is_empty() {
        out.push(build_one_list(&mut entries, 0));
    }
    out
}

/// Consumes the leading run of `entries` that shares the shallowest level and numbering, folding any
/// deeper run that follows an item into that item as a nested list. The consumed entries are removed
/// from the front of the deque so their block content is moved into the tree rather than cloned.
fn build_one_list(entries: &mut VecDeque<ListEntry>, depth: usize) -> Block {
    const MAX_LIST_DEPTH: usize = 256;
    let base = entries.front().map_or(0, |entry| entry.level);
    let num_id = entries.front().map_or(0, |entry| entry.num_id);
    let numbering = entries.front().and_then(|entry| entry.numbering.clone());
    let mut items: Vec<Vec<Block>> = Vec::new();
    while entries
        .front()
        .is_some_and(|entry| entry.level == base && entry.num_id == num_id)
    {
        let Some(entry) = entries.pop_front() else {
            break;
        };
        let mut item = vec![entry.block];
        if matches!(entries.front(), Some(next) if next.level > base) && depth < MAX_LIST_DEPTH {
            item.push(build_one_list(entries, depth + 1));
        }
        items.push(item);
    }
    match numbering {
        Some(attrs) => Block::OrderedList(attrs, items),
        None => Block::BulletList(items),
    }
}
