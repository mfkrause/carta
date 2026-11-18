//! `#+INCLUDE:` expansion: the named files are spliced into the text before it is parsed.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use carta_core::{Error, HostAccess, ReaderOptions, Result};

use super::blocks::headline_level;
use super::greater::greater_block_open;
use super::keyword::keyword_line;

/// Replaces every `#+INCLUDE: "file"` line of `text` with the content of the file it names. A
/// plain include splices the file's own markup, nested includes resolved; `src`, `example`, and
/// `export` wrap the file in the block of that kind. A file that cannot be read contributes
/// nothing.
///
/// # Errors
/// A file that includes itself, directly or through other files, and an expansion out of all
/// proportion to the input.
pub(super) fn expand<'a>(text: &'a str, options: &ReaderOptions) -> Result<Cow<'a, str>> {
    if !text.lines().any(|line| Include::parse(line).is_some()) {
        return Ok(Cow::Borrowed(text));
    }
    let sources = options.source_paths.values();
    let mut expander = Expander {
        host: options.host,
        chain: sources
            .filter_map(|path| options.host.canonicalize(path).ok())
            .collect(),
        known: BTreeSet::new(),
        work: (1usize << 20).saturating_add(text.len().saturating_mul(64)),
        closing: false,
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    expander.expand(&lines, options.source_dir.as_deref(), &mut out)?;
    Ok(Cow::Owned(out.join("\n")))
}

struct Expander {
    host: HostAccess,
    /// Canonical paths of the document's own files, then of each file being included.
    chain: Vec<PathBuf>,
    /// The files read so far; a file adds to the work budget once.
    known: BTreeSet<PathBuf>,
    /// Remaining expanded text work; each inclusion spends the length of its file.
    work: usize,
    /// Set after included markup: the next line of text opens a new block.
    closing: bool,
}

impl Expander {
    fn expand(
        &mut self,
        lines: &[&str],
        directory: Option<&Path>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        let mut blocks = BlockTracker::default();
        for line in lines {
            let verbatim = blocks.holds(line);
            if let Some(include) = Include::parse(line).filter(|_| !verbatim) {
                self.include(&include, directory, out)?;
            } else {
                self.separate(line, out);
                out.push((*line).to_owned());
            }
        }
        Ok(())
    }

    /// Keeps included markup a block of its own: a blank line parts it from text that touches it.
    fn separate(&mut self, next: &str, out: &mut Vec<String>) {
        if std::mem::take(&mut self.closing) && !next.trim().is_empty() {
            out.push(String::new());
        }
    }

    fn include(
        &mut self,
        include: &Include<'_>,
        directory: Option<&Path>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        let path = directory.map_or_else(
            || PathBuf::from(include.file),
            |directory| directory.join(include.file),
        );
        let file = self.read(&path)?;
        let text = file.as_ref().map_or("", |(_, text)| text.as_str());
        let lines: Vec<&str> = text.lines().collect();
        let lines = include.lines.select(&lines);
        let indent = include.indent;
        let mut wrap = |open: &str, close: &str| {
            out.push(format!("{indent}{open}"));
            out.extend(lines.iter().map(|line| format!("{indent}{}", escape(line))));
            out.push(format!("{indent}{close}"));
        };
        match include.kind {
            Kind::Source(language) => wrap(&format!("#+begin_src {language}"), "#+end_src"),
            Kind::Example => wrap("#+begin_example", "#+end_example"),
            Kind::Export(Some(format)) => wrap(&format!("#+begin_export {format}"), "#+end_export"),
            Kind::Export(None) => {}
            Kind::Markup => {
                let Some((canonical, _)) = &file else {
                    return Ok(());
                };
                if self.chain.contains(canonical) {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("include file loop at {}", include.file),
                    )));
                }
                let mut included = Vec::new();
                self.chain.push(canonical.clone());
                self.closing = false;
                let expanded = self.expand(lines, path.parent(), &mut included);
                self.chain.pop();
                expanded?;
                shift_headlines(&mut included, include.minimum_level);
                self.closing = out.last().is_some_and(|line| !line.trim().is_empty());
                self.separate(included.first().map_or("", String::as_str), out);
                out.extend(included.into_iter().map(|line| {
                    if line.is_empty() {
                        line
                    } else {
                        format!("{indent}{line}")
                    }
                }));
                self.closing = true;
            }
        }
        Ok(())
    }

    /// The canonical path and text of a file, with its line endings normalized and its length
    /// charged to the work budget. `Ok(None)` when the file cannot be read.
    fn read(&mut self, path: &Path) -> Result<Option<(PathBuf, String)>> {
        let Ok(canonical) = self.host.canonicalize(path) else {
            return Ok(None);
        };
        let Ok(text) = self.host.read_to_string(&canonical) else {
            return Ok(None);
        };
        if self.known.insert(canonical.clone()) {
            self.work = self.work.saturating_add(text.len().saturating_mul(64));
        }
        self.work = self
            .work
            .checked_sub(text.len().max(1))
            .ok_or(Error::OutputTooLarge)?;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        Ok(Some((canonical, text)))
    }
}

/// A parsed `#+INCLUDE: "file" [kind] [:minlevel N] [:lines "A-B"]` line.
struct Include<'a> {
    /// The whitespace before the keyword, repeated before each included line.
    indent: &'a str,
    file: &'a str,
    kind: Kind<'a>,
    minimum_level: Option<usize>,
    lines: LineRange,
}

enum Kind<'a> {
    /// The file is markup of its own, parsed in place.
    Markup,
    /// The file is the body of a source block in the given language (possibly none).
    Source(&'a str),
    Example,
    /// The file is raw output in the given format; without a format it is dropped.
    Export(Option<&'a str>),
}

impl<'a> Include<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let (key, _) = keyword_line(line)?;
        if !key.eq_ignore_ascii_case("include") {
            return None;
        }
        let indent = line.get(..line.len() - line.trim_start().len())?;
        let (_, value) = line.split_once(':')?;
        let (file, arguments) = value.trim_start().strip_prefix('"')?.split_once('"')?;
        let mut words = arguments.split_whitespace().peekable();
        let mut argument = || words.next_if(|word| !word.starts_with(':'));
        let kind = match argument() {
            Some("src") => Kind::Source(argument().unwrap_or("")),
            Some("example") => Kind::Example,
            Some("export") => Kind::Export(argument()),
            _ => Kind::Markup,
        };
        let mut minimum_level = None;
        let mut lines = LineRange::ALL;
        while let Some(word) = words.next() {
            match word {
                ":minlevel" => minimum_level = words.next().and_then(|level| level.parse().ok()),
                ":lines" => lines = LineRange::parse(words.next().unwrap_or("")),
                _ => {}
            }
        }
        Some(Self {
            indent,
            file,
            kind,
            minimum_level,
            lines,
        })
    }
}

/// The lines a `:lines "A-B"` option keeps: one-based, from `A` up to but not including `B`.
struct LineRange {
    start: usize,
    end: usize,
}

impl LineRange {
    const ALL: Self = Self {
        start: 1,
        end: usize::MAX,
    };

    /// Parses `"A-B"`, where either bound may be absent. Anything else keeps no line.
    fn parse(range: &str) -> Self {
        let Some((start, end)) = range.trim_matches('"').split_once('-') else {
            return Self { start: 1, end: 1 };
        };
        Self {
            start: start.parse().unwrap_or(1),
            end: end.parse().unwrap_or(usize::MAX),
        }
    }

    fn select<'l, 't>(&self, lines: &'l [&'t str]) -> &'l [&'t str] {
        let end = self.end.saturating_sub(1).min(lines.len());
        lines.get(self.start.saturating_sub(1)..end).unwrap_or(&[])
    }
}

/// Tracks the `#+begin_…`/`#+end_…` regions whose lines are verbatim content, never keywords.
#[derive(Default)]
struct BlockTracker {
    /// The open block's name and how many blocks of that name are nested.
    open: Option<(String, usize)>,
}

impl BlockTracker {
    /// Whether `line` lies inside a verbatim block; the line that opens one is still outside.
    fn holds(&mut self, line: &str) -> bool {
        let opened = greater_block_open(line);
        let Some((name, depth)) = &mut self.open else {
            self.open = opened
                .filter(|name| {
                    ["src", "example", "export", "comment"]
                        .iter()
                        .any(|verbatim| name.eq_ignore_ascii_case(verbatim))
                })
                .map(|name| (name, 1));
            return false;
        };
        if opened.is_some_and(|opened| opened.eq_ignore_ascii_case(name)) {
            *depth += 1;
        } else if line
            .trim_start()
            .eq_ignore_ascii_case(&format!("#+end_{name}"))
        {
            *depth -= 1;
        }
        if *depth == 0 {
            self.open = None;
        }
        true
    }
}

/// Moves every headline of an included file by the same number of levels, so its shallowest
/// headline sits at `minimum_level`.
fn shift_headlines(lines: &mut [String], minimum_level: Option<usize>) {
    let Some(minimum_level) = minimum_level else {
        return;
    };
    let mut blocks = BlockTracker::default();
    let headlines: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !blocks.holds(line))
        .filter_map(|(index, line)| Some((index, headline_level(line)?)))
        .collect();
    let Some(shallowest) = headlines.iter().map(|(_, level)| *level).min() else {
        return;
    };
    for (index, _) in headlines {
        let Some(line) = lines.get_mut(index) else {
            continue;
        };
        if minimum_level >= shallowest {
            line.insert_str(0, &"*".repeat(minimum_level - shallowest));
        } else {
            line.drain(..shallowest - minimum_level);
        }
    }
}

/// Guards a line of included verbatim text with a comma when it would read as a headline or a
/// keyword, the escape a block's content loses again when it is read.
fn escape(line: &str) -> Cow<'_, str> {
    let content = line.trim_start().trim_start_matches(',');
    if content.starts_with('*') || content.starts_with("#+") {
        let indent = line.len() - line.trim_start().len();
        let mut escaped = line.to_owned();
        escaped.insert(indent, ',');
        Cow::Owned(escaped)
    } else {
        Cow::Borrowed(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_range_is_one_based_and_excludes_its_end() {
        let lines = ["one", "two", "three"];
        let select = |range: &str| LineRange::parse(range).select(&lines).join(" ");
        assert_eq!(select("\"2-3\""), "two");
        assert_eq!(select("\"2-\""), "two three");
        assert_eq!(select("\"-2\""), "one");
        assert_eq!(select("\"2\""), "");
        assert_eq!(select("\"3-2\""), "");
    }

    #[test]
    fn a_minimum_level_moves_every_headline_outside_blocks() {
        let shifted = |minimum_level| {
            let mut lines: Vec<String> = [
                "** two",
                "#+begin_src org",
                "* code",
                "#+end_src",
                "*** three",
            ]
            .map(str::to_owned)
            .into();
            shift_headlines(&mut lines, Some(minimum_level));
            lines.join("|")
        };
        assert_eq!(
            shifted(3),
            "*** two|#+begin_src org|* code|#+end_src|**** three"
        );
        assert_eq!(
            shifted(1),
            "* two|#+begin_src org|* code|#+end_src|** three"
        );
    }
}
