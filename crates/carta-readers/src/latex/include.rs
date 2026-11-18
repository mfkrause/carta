//! Files a LaTeX document pulls in: `\input`, `\include`, `\subfile`, local packages, and listings.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use carta_ast::{Attr, Block};
use carta_core::{HostAccess, ReaderOptions};

use super::tables::parse_key_values;
use super::{Expansion, Frame, Macro, MacroCall, Parser, split_document};
use crate::tabs::expand_tabs;

/// Where the files a document names are found, and which have been read.
pub(super) struct Files {
    host: HostAccess,
    /// The directories searched, in order; empty searches the working directory.
    search_path: Vec<PathBuf>,
    /// Canonical paths read so far; each extends the expansion budget once.
    read: RefCell<BTreeSet<PathBuf>>,
    /// The document's own files: the character each starts on and its canonical path, if any.
    sources: Vec<(usize, Option<PathBuf>)>,
    /// The source file that holds the position being read.
    current: Cell<usize>,
}

/// The expansion in progress while `canonical` is being read, so a file cannot include itself.
fn input_call(canonical: &Path) -> MacroCall {
    (
        "input".to_owned(),
        Rc::new(Macro::Token(String::new())),
        vec![canonical.to_string_lossy().into_owned()],
    )
}

impl Parser {
    /// Lets the document `input` read the files it names. A source file of the document counts
    /// as being read already while the position being read lies in it.
    pub(super) fn read_files(&mut self, input: &str, options: &ReaderOptions) {
        let mut character = 0;
        let sources = options.source_files(input).into_iter().map(|file| {
            let first = character;
            character += expand_tabs(file.text, options.tab_stop).chars().count();
            let path = file
                .path
                .and_then(|path| options.host.canonicalize(path).ok());
            (first, path)
        });
        self.files = Some(Rc::new(Files {
            host: options.host,
            search_path: options.resource_path.clone(),
            read: RefCell::new(BTreeSet::new()),
            sources: sources.collect(),
            current: Cell::new(0),
        }));
    }

    /// Notes which source file holds the position being read. Only the parser of the document's
    /// own text knows that position; any other parser keeps the file its maker noted.
    pub(super) fn enter_source_file(&self) {
        let (Some(files), Some(offset)) = (&self.files, self.document_offset) else {
            return;
        };
        let position = offset + self.frames.first().map_or(0, |frame| frame.pos);
        let holds = |(first, _): &(usize, Option<PathBuf>)| *first <= position;
        files
            .current
            .set(files.sources.iter().rposition(holds).unwrap_or(0));
    }

    /// Whether `canonical` is the source file that holds the position being read.
    fn in_source_file(&self, canonical: &Path) -> bool {
        self.enter_source_file();
        let current = self
            .files
            .as_ref()
            .and_then(|files| files.sources.get(files.current.get()));
        current.is_some_and(|(_, path)| path.as_deref() == Some(canonical))
    }

    /// The canonical path and text of the file `name`, charged to the expansion budget. `None`
    /// when the file cannot be read or the budget is spent.
    fn read_file(&self, name: &str) -> Option<(PathBuf, String)> {
        let files = self.files.as_ref()?;
        let found = files.host.read_from(&files.search_path, Path::new(name));
        let (canonical, text) = found.ok()?;
        let budget = &self.expansions;
        if files.read.borrow_mut().insert(canonical.clone()) {
            let grant = text.len().saturating_mul(64);
            budget.calls.set(budget.calls.get().saturating_add(grant));
            budget
                .characters
                .set(budget.characters.get().saturating_add(grant));
        }
        budget
            .characters
            .set(budget.characters.get().checked_sub(text.len())?);
        Some((canonical, text))
    }

    /// Pushes the text of the file an `\input`, `\include`, or `\subfile` names before the
    /// remaining input, and loads the packages a `\usepackage` or `\RequirePackage` names. A file
    /// that cannot be read, or is being read already, adds nothing.
    pub(super) fn expand_input(&mut self) -> bool {
        if self.files.is_none() {
            return false;
        }
        self.drop_exhausted_frames();
        if self.frames.last().is_some_and(|frame| frame.fixed_token) {
            return false;
        }
        let Some(command) = self.peek_control_word() else {
            return false;
        };
        if matches!(command.as_str(), "usepackage" | "RequirePackage") {
            self.consume_control_word();
            self.load_packages();
            return true;
        }
        if !matches!(command.as_str(), "input" | "include" | "subfile") {
            return false;
        }
        let parent = self.frames.last().and_then(|frame| frame.expansion.clone());
        self.consume_control_word();
        let Some(argument) = self.read_group_raw() else {
            return true;
        };
        let mut name = argument.trim().to_owned();
        if Path::new(&name).extension().is_none() {
            name.push_str(".tex");
        }
        let Some((canonical, text)) = self.read_file(&name) else {
            return true;
        };
        let call = input_call(&canonical);
        if self.in_source_file(&canonical)
            || !self.expansions.active.borrow_mut().insert(call.clone())
        {
            return true;
        }
        let expansion = Rc::new(Expansion {
            call,
            state: Rc::clone(&self.expansions),
            parent,
        });
        let text = if command == "subfile" {
            split_document(&text).1
        } else {
            &text
        };
        if !text.is_empty() {
            self.frames.push(Frame::new(text, Some(expansion)));
        }
        true
    }

    /// Reads the argument of a `\usepackage` or `\RequirePackage` and adopts the macros that each
    /// named package found beside the document defines, and those of the packages it loads in
    /// turn. Nothing else of a package is kept. A package that is being read is not read again.
    fn load_packages(&mut self) {
        while self.read_optional_raw().is_some() {}
        let names = self.read_group_raw().unwrap_or_default();
        for name in names.split(',') {
            let Some((canonical, text)) = self.read_file(&format!("{}.sty", name.trim())) else {
                continue;
            };
            let call = input_call(&canonical);
            if !self.expansions.active.borrow_mut().insert(call.clone()) {
                continue;
            }
            let mut package = self.child(&text, false);
            package.expand_source(true);
            self.macros = package.macros;
            self.expansions.active.borrow_mut().remove(&call);
        }
        while self.read_optional_raw().is_some() {}
    }

    /// A `\lstinputlisting[options]{file}` or `\inputminted[options]{language}{file}`: the file
    /// as a code block. A file that cannot be read leaves the block empty.
    pub(super) fn parse_listing_file(&mut self, command: &str) -> Block {
        self.consume_control_word();
        let options = self.read_optional_raw().unwrap_or_default();
        let options = parse_key_values(&options);
        let option = |key: &str| {
            let found = options.iter().find(|(name, _)| name == key);
            found.map(|(_, value)| value.as_str())
        };
        let minted = command == "inputminted";
        let language = if minted {
            self.read_group_raw()
        } else {
            option("language").map(str::to_owned)
        };
        let file = self.read_group_raw().unwrap_or_default();
        let text = self.read_file(file.trim()).map(|(_, text)| text);
        let mut text = text.unwrap_or_default();
        if !minted {
            let line = |key: &str| option(key).and_then(|line| line.parse::<usize>().ok());
            let first = line("firstline").unwrap_or(1).saturating_sub(1);
            let count = line("lastline").map_or(usize::MAX, |last| last.saturating_sub(first));
            let lines: Vec<&str> = text.lines().skip(first).take(count).collect();
            text = lines.join("\n");
        }
        let language = language.filter(|language| !language.is_empty());
        Block::CodeBlock(
            Box::new(Attr {
                classes: language
                    .map(|language| language.to_lowercase().into())
                    .into_iter()
                    .collect(),
                attributes: options
                    .iter()
                    .map(|(key, value)| (key.as_str().into(), value.as_str().into()))
                    .collect(),
                ..Attr::default()
            }),
            text.into(),
        )
    }
}
