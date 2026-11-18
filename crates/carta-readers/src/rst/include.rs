//! Files a document pulls in: the `include` directive and the `file` and `url` options.

use carta_ast::{Attr, Block};
use carta_core::{Error, HostAccess, ReaderOptions};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The files `include` directives pull in, shared by the definition pass and the block pass.
pub(super) struct Includes<'a> {
    host: HostAccess,
    /// Remaining expanded text work; each inclusion spends the length of its file.
    work: &'a Cell<Option<usize>>,
    /// The document's own source files, each with the document line it starts on.
    files: Vec<(usize, Active)>,
    /// The source file that holds the document line being read.
    current: Cell<usize>,
    /// The directories searched for a data file; empty searches the working directory.
    resource_path: Vec<PathBuf>,
    /// The files being included, outermost first.
    chain: RefCell<Vec<Active>>,
    /// File texts by canonical path.
    texts: RefCell<BTreeMap<PathBuf, Rc<str>>>,
}

/// A file whose lines are being read. Standard input has no path.
struct Active {
    canonical: Option<PathBuf>,
    directory: Option<PathBuf>,
}

/// A file opened for inclusion. It stays on the include chain until dropped.
pub(super) struct Included<'a> {
    includes: &'a Includes<'a>,
    /// The file's lines, narrowed by the directive's line options.
    pub(super) lines: Vec<String>,
}

impl Drop for Included<'_> {
    fn drop(&mut self) {
        self.includes.chain.borrow_mut().pop();
    }
}

impl<'a> Includes<'a> {
    pub(super) fn new(input: &str, options: &ReaderOptions, work: &'a Cell<Option<usize>>) -> Self {
        let mut line = 0;
        let files = options.source_files(input).into_iter().map(|file| {
            let first = line;
            line += line_breaks(file.text);
            let path = file
                .path
                .and_then(|path| options.host.canonicalize(path).ok());
            let directory = file.directory.map(Path::to_path_buf);
            (
                first,
                Active {
                    canonical: path,
                    directory,
                },
            )
        });
        Self {
            host: options.host,
            work,
            files: files.collect(),
            current: Cell::new(0),
            resource_path: options.resource_path.clone(),
            chain: RefCell::new(Vec::new()),
            texts: RefCell::new(BTreeMap::new()),
        }
    }

    /// Resolves the includes that follow against the source file that holds document line `line`.
    pub(super) fn enter_line(&self, line: usize) {
        let file = self.files.iter().rposition(|(first, _)| *first <= line);
        self.current.set(file.unwrap_or(0));
    }

    fn source(&self) -> Option<&Active> {
        self.files.get(self.current.get()).map(|(_, file)| file)
    }

    /// Opens the file an `include` directive names, to parse its lines in place. `Ok(None)` when
    /// the file cannot be read.
    ///
    /// # Errors
    /// A file that includes itself, directly or through other files, and includes nested too deep.
    pub(super) fn open(
        &'a self,
        argument: &str,
        options: &[(String, String)],
    ) -> Result<Option<Included<'a>>, Error> {
        let Some((path, canonical)) = self.locate(argument) else {
            return Ok(None);
        };
        let chain = self.chain.borrow();
        let mut active = self.source().into_iter().chain(chain.iter());
        if active.any(|active| active.canonical.as_ref() == Some(&canonical)) {
            return Err(invalid(format!("include file loop at {}", argument.trim())));
        }
        if chain.len() >= HostAccess::NESTING_LIMIT {
            return Err(invalid("include nesting limit exceeded".to_owned()));
        }
        drop(chain);
        let Some(lines) = self.lines(&canonical, options) else {
            return Ok(None);
        };
        self.chain.borrow_mut().push(Active {
            canonical: Some(canonical),
            directory: path.parent().map(Path::to_path_buf),
        });
        Ok(Some(Included {
            includes: self,
            lines,
        }))
    }

    /// The lines of the file an `include` directive names, to quote as a code block. Empty when
    /// the file cannot be read.
    pub(super) fn quote(&self, argument: &str, options: &[(String, String)]) -> Vec<String> {
        self.locate(argument)
            .and_then(|(_, canonical)| self.lines(&canonical, options))
            .unwrap_or_default()
    }

    /// The path an include names, resolved against the including file's directory, and its
    /// canonical form. `None` when no such file exists.
    fn locate(&self, argument: &str) -> Option<(PathBuf, PathBuf)> {
        let argument = argument.trim();
        if argument.is_empty() {
            return None;
        }
        let chain = self.chain.borrow();
        let including = chain.last().or_else(|| self.source());
        let directory = including.and_then(|active| active.directory.as_deref());
        let path = resolve(directory, argument);
        let canonical = self.host.canonicalize(&path).ok()?;
        Some((path, canonical))
    }

    /// The text a directive's `file` option (searched along the resource path) or `url` option
    /// names. `Ok(None)` when it has neither.
    ///
    /// # Errors
    /// A file or URL that cannot be read.
    pub(super) fn external(&self, options: &[(String, String)]) -> Result<Option<Rc<str>>, Error> {
        let named = |name: &str, error: io::Error| {
            Error::Io(io::Error::new(error.kind(), format!("{name}: {error}")))
        };
        let text = if let Some(file) = option(options, "file") {
            self.resource(Path::new(file))
                .map_err(|error| named(file, error))?
        } else if let Some(url) = option(options, "url") {
            let item = self.host.fetch(url).map_err(|error| named(url, error))?;
            let text: Rc<str> = String::from_utf8(item.bytes)
                .map_err(|error| named(url, io::Error::new(io::ErrorKind::InvalidData, error)))?
                .into();
            self.grant(&text);
            text
        } else {
            return Ok(None);
        };
        Ok(Some(if self.spend(&text) { text } else { "".into() }))
    }

    fn resource(&self, name: &Path) -> io::Result<Rc<str>> {
        let working_directory = [PathBuf::new()];
        let directories = if self.resource_path.is_empty() {
            &working_directory[..]
        } else {
            &self.resource_path
        };
        let mut found = Err(io::ErrorKind::NotFound.into());
        for directory in directories {
            found = self.host.canonicalize(&directory.join(name));
            if found.is_ok() {
                break;
            }
        }
        self.text(&found?)
    }

    fn lines(&self, canonical: &Path, options: &[(String, String)]) -> Option<Vec<String>> {
        let text = self.text(canonical).ok()?;
        if !self.spend(&text) {
            return None;
        }
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        Some(select(&lines, options).to_vec())
    }

    /// A file's text with its line endings normalized, read once so both passes see the same
    /// lines.
    fn text(&self, canonical: &Path) -> io::Result<Rc<str>> {
        if let Some(text) = self.texts.borrow().get(canonical) {
            return Ok(Rc::clone(text));
        }
        let text: Rc<str> = self
            .host
            .read_to_string(canonical)?
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .into();
        self.grant(&text);
        self.texts
            .borrow_mut()
            .insert(canonical.to_path_buf(), Rc::clone(&text));
        Ok(text)
    }

    /// Extends the work budget for a text seen for the first time.
    fn grant(&self, text: &str) {
        self.work.set(
            self.work
                .get()
                .map(|work| work.saturating_add(text.len().saturating_mul(64))),
        );
    }

    /// Charges one use of `text` to the work budget; `false` once the budget is spent.
    fn spend(&self, text: &str) -> bool {
        let remaining = self
            .work
            .get()
            .and_then(|work| work.checked_sub(text.len().max(1)));
        self.work.set(remaining);
        remaining.is_some()
    }
}

fn invalid(message: String) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}

/// How many line ends `text` holds: a line feed, a carriage return, or the pair of them.
fn line_breaks(text: &str) -> usize {
    let pairs = text.matches("\r\n").count();
    text.matches(['\r', '\n']).count() - pairs
}

fn resolve(directory: Option<&Path>, argument: &str) -> PathBuf {
    directory.map_or_else(
        || PathBuf::from(argument),
        |directory| directory.join(argument),
    )
}

fn option<'a>(options: &'a [(String, String)], key: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Whether an include's options ask for the file as a code block instead of parsed markup.
pub(super) fn is_literal(options: &[(String, String)]) -> bool {
    option(options, "literal").is_some() || option(options, "code").is_some()
}

/// The lines an include keeps: `start-line` and `end-line` bound a one-based inclusive range
/// (negative numbers count from the end), then `start-after` drops everything through the first
/// line holding its text and `end-before` everything from the first line holding its text.
fn select<'l>(lines: &'l [String], options: &[(String, String)]) -> &'l [String] {
    let count = i64::try_from(lines.len()).unwrap_or(i64::MAX);
    let line = |key: &str| {
        let number: i64 = option(options, key)?.parse().ok()?;
        let position = if number < 0 { count + number } else { number };
        Some(usize::try_from(position.clamp(0, count)).unwrap_or(0))
    };
    let start = line("start-line").map_or(0, |start| start.saturating_sub(1));
    let end = line("end-line").unwrap_or(lines.len());
    let mut kept = lines.get(start..end).unwrap_or(&[]);
    if let Some(marker) = option(options, "start-after") {
        let after = kept
            .iter()
            .position(|line| line.contains(marker))
            .map_or(kept.len(), |index| index + 1);
        kept = kept.get(after..).unwrap_or(&[]);
    }
    if let Some(marker) = option(options, "end-before")
        && let Some(before) = kept.iter().position(|line| line.contains(marker))
    {
        kept = kept.get(..before).unwrap_or(&[]);
    }
    kept
}

/// The code block a literal include produces from the lines it kept. The first class is the
/// language (empty when none is named); the remaining options ride along as attributes.
pub(super) fn literal_block(options: &[(String, String)], lines: &[String]) -> Block {
    let language = option(options, "code")
        .or_else(|| option(options, "literal"))
        .unwrap_or("");
    let mut id = "";
    let mut classes = vec![language.to_owned()];
    let mut attributes = Vec::new();
    let mut start_from = None;
    for (key, value) in options {
        match key.as_str() {
            "name" => id = value,
            "class" => {}
            "number-lines" => {
                classes.push("numberLines".to_owned());
                start_from = Some(value).filter(|value| !value.is_empty());
            }
            _ => attributes.push((key.as_str(), value)),
        }
    }
    attributes.extend(start_from.map(|value| ("startFrom", value)));
    classes.extend(
        options
            .iter()
            .filter(|(key, _)| key == "class")
            .flat_map(|(_, value)| value.split_whitespace().map(str::to_owned)),
    );
    let text: String = lines
        .iter()
        .flat_map(|line| [line.as_str(), "\n"])
        .collect();
    Block::CodeBlock(
        Box::new(Attr {
            id: id.into(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(key, value)| (key.into(), field_value(value).into()))
                .collect(),
        }),
        text.into(),
    )
}

/// An option's value as an attribute: a value that is set keeps the blank line closing its field.
fn field_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("{value}\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(count: usize) -> Vec<String> {
        (1..=count).map(|number| format!("line{number}")).collect()
    }

    fn selected(options: &[(&str, &str)]) -> String {
        let options: Vec<(String, String)> = options
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        select(&lines(7), &options).join(" ")
    }

    #[test]
    fn line_options_bound_a_one_based_inclusive_range() {
        assert_eq!(
            selected(&[("start-line", "2"), ("end-line", "4")]),
            "line2 line3 line4"
        );
        assert_eq!(selected(&[("start-line", "0"), ("end-line", "1")]), "line1");
        assert_eq!(selected(&[("start-line", "-2")]), "line5 line6 line7");
        assert_eq!(selected(&[("end-line", "-5")]), "line1 line2");
        assert_eq!(selected(&[("start-line", "5"), ("end-line", "2")]), "");
        assert_eq!(
            selected(&[("start-line", "-100"), ("end-line", "100")]),
            lines(7).join(" ")
        );
        assert_eq!(selected(&[("start-line", "x")]), lines(7).join(" "));
    }

    #[test]
    fn markers_cut_whole_lines_inside_the_line_range() {
        assert_eq!(
            selected(&[("start-after", "ne2"), ("end-before", "ne5")]),
            "line3 line4"
        );
        assert_eq!(selected(&[("start-after", "missing")]), "");
        assert_eq!(selected(&[("end-before", "missing")]), lines(7).join(" "));
        assert_eq!(
            selected(&[("start-line", "4"), ("start-after", "line2")]),
            ""
        );
    }
}
