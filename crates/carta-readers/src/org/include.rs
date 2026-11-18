//! `#+INCLUDE:` expansion: the named files are spliced into the text before it is parsed.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use carta_core::{Error, HostAccess, ReaderOptions, Result};

use super::blocks::headline_level;
use super::greater::greater_block_open;
use super::inline::is_verbatim_marker;
use super::inline_helpers::{post_ok, pre_ok};
use super::keyword::keyword_line;
use super::{collect_footnotes, footnote_definition, is_footnote_label_char, normalize};

/// Normalizes the line endings of `input` and replaces every `#+INCLUDE: "file"` line with the
/// content of the file it names. A plain include splices the file's own markup, nested includes
/// resolved, with its headlines below the headline that the include is under; `src`, `example`,
/// and `export` wrap the file in the block of that kind. A file that cannot be read contributes
/// nothing.
///
/// # Errors
/// A file that includes itself, directly or through other files, includes nested too deep, and an
/// expansion out of all proportion to the input.
pub(super) fn expand<'a>(input: &'a str, options: &ReaderOptions) -> Result<Cow<'a, str>> {
    let normalized = normalize(input);
    if !normalized
        .lines()
        .any(|line| Include::parse(line).is_some())
    {
        return Ok(normalized);
    }
    let mut expander = Expander {
        host: options.host,
        chain: Vec::new(),
        known: BTreeSet::new(),
        local_labels: BTreeSet::new(),
        work: (1usize << 20).saturating_add(input.len().saturating_mul(64)),
        closing: false,
    };
    // The source files are one text: a block or a section of one continues in the next.
    let mut context = Context::default();
    let mut output = String::new();
    for file in options.source_files(input) {
        let path = file
            .path
            .and_then(|path| options.host.canonicalize(path).ok());
        expander.chain = path.into_iter().collect();
        let text = normalize(file.text);
        let lines: Vec<&str> = text.split('\n').collect();
        let mut out = Vec::new();
        expander.expand(&lines, file.directory, &mut context, &mut out)?;
        output.push_str(&out.join("\n"));
    }
    Ok(Cow::Owned(output))
}

struct Expander {
    host: HostAccess,
    /// The canonical path of the source file being expanded, then of each file being included.
    chain: Vec<PathBuf>,
    /// The files read so far; a file adds to the work budget once.
    known: BTreeSet<PathBuf>,
    /// Remaining expanded text work; each inclusion spends the length of its file.
    work: usize,
    /// Set after included markup: the next line of text opens a new block.
    closing: bool,
    /// The footnote labels made for the references inside included files.
    local_labels: BTreeSet<String>,
}

/// What the lines of a text that were read establish for the lines that follow.
#[derive(Default)]
struct Context {
    /// The verbatim block that is open.
    blocks: BlockTracker,
    /// The level of the text's own headline that the next line is below.
    level: Option<usize>,
    /// The footnote labels that the text itself has defined.
    footnotes: BTreeSet<String>,
}

impl Expander {
    fn expand(
        &mut self,
        lines: &[&str],
        directory: Option<&Path>,
        context: &mut Context,
        out: &mut Vec<String>,
    ) -> Result<()> {
        for line in lines {
            let verbatim = context.blocks.holds(line);
            if let Some(include) = Include::parse(line).filter(|_| !verbatim) {
                self.include(&include, directory, context, out)?;
            } else {
                if !verbatim {
                    context.level = headline_level(line).or(context.level);
                    let defined = footnote_definition(line).map(|(label, _)| label);
                    context.footnotes.extend(defined);
                }
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
        context: &mut Context,
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
        let block = |open: &str, close: &str| {
            let body = lines.iter().map(|line| escape(line).into_owned());
            let open = std::iter::once(open.to_owned());
            open.chain(body).chain([close.to_owned()]).collect()
        };
        let produced: Vec<String> = match include.kind {
            Kind::Source(language) => block(&format!("#+begin_src {language}"), "#+end_src"),
            Kind::Example => block("#+begin_example", "#+end_example"),
            Kind::Export(Some(format)) => {
                block(&format!("#+begin_export {format}"), "#+end_export")
            }
            Kind::Export(None) => Vec::new(),
            Kind::Markup => {
                let Some((canonical, _)) = &file else {
                    return Ok(());
                };
                if self.chain.contains(canonical) {
                    return Err(invalid(format!("include file loop at {}", include.file)));
                }
                if self.chain.len() > HostAccess::NESTING_LIMIT {
                    return Err(invalid("include nesting limit exceeded".to_owned()));
                }
                let mut included = Vec::new();
                self.chain.push(canonical.clone());
                self.closing = false;
                let mut inner = Context::default();
                let expanded = self.expand(lines, path.parent(), &mut inner, &mut included);
                self.chain.pop();
                expanded?;
                // The headlines of the file become children of the headline above the include.
                let below = context.level.map(|level| level + 1);
                let minimum_level = include.minimum_level.or(below);
                shift_headlines(&mut included, minimum_level, &mut self.work)?;
                if include.indent.is_empty() {
                    self.localize_footnotes(&mut included, &context.footnotes)?;
                }
                if let Some((directory, _)) = include.file.rsplit_once('/') {
                    self.rebase_links(&mut included, directory)?;
                }
                self.closing = out.last().is_some_and(|line| !line.trim().is_empty());
                self.separate(included.first().map_or("", String::as_str), out);
                self.closing = true;
                included
            }
        };
        let indent = include.indent;
        let indented = produced.iter().filter(|line| !line.is_empty()).count();
        spend(&mut self.work, indent.len().saturating_mul(indented))?;
        out.extend(produced.into_iter().map(|line| {
            if line.is_empty() {
                line
            } else {
                format!("{indent}{line}")
            }
        }));
        Ok(())
    }

    /// Gives the footnotes that included markup defines labels of their own, so its references
    /// keep to its own notes when another file uses the same label. The last definition of each
    /// label is repeated under the label as written, for the references outside the file, unless
    /// the including text has `defined` that label itself.
    fn localize_footnotes(
        &mut self,
        lines: &mut Vec<String>,
        defined: &BTreeSet<String>,
    ) -> Result<()> {
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (_, definitions) = collect_footnotes(&borrowed);
        let last: BTreeMap<String, String> = definitions
            .into_iter()
            .filter(|(label, _)| !self.local_labels.contains(label))
            .collect();
        if last.is_empty() {
            return Ok(());
        }
        let prefix = format!("include-{}-", self.local_labels.len());
        let relabel = |line: &str| outside_code(line, |text| relabel(text, &last, &prefix));
        let mut blocks = BlockTracker::default();
        for line in lines.iter_mut() {
            if !blocks.holds(line) {
                *line = relabel(line);
            }
        }
        // Two blank lines end a definition, so the text after the include is not part of one.
        lines.push(String::new());
        for (label, body) in &last {
            self.local_labels.insert(format!("{prefix}{label}"));
            if defined.contains(label) {
                continue;
            }
            let definition = format!("[fn:{label}] {}", relabel(body));
            spend(&mut self.work, definition.len())?;
            lines.extend(definition.split('\n').map(str::to_owned));
        }
        lines.extend([String::new(), String::new()]);
        Ok(())
    }

    /// Puts `directory`, where an included file lies as seen from the including file, before the
    /// relative file targets of the file's links, so each link still names the same file.
    fn rebase_links(&mut self, lines: &mut [String], directory: &str) -> Result<()> {
        let mut blocks = BlockTracker::default();
        for line in lines {
            if blocks.holds(line) || !(line.contains("[[") || line.contains("<file:")) {
                continue;
            }
            let rebased = outside_code(line, |text| rebase_links(text, directory));
            spend(&mut self.work, rebased.len().saturating_sub(line.len()))?;
            *line = rebased;
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
        spend(&mut self.work, text.len().max(1))?;
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
                ":lines" => {
                    // A quoted range can hold spaces, which split it into several words.
                    let mut range = String::new();
                    for word in words.by_ref() {
                        range.push_str(word);
                        if !range.starts_with('"') || range.len() > 1 && range.ends_with('"') {
                            break;
                        }
                    }
                    if range.starts_with('"') {
                        lines = LineRange::parse(&range);
                    }
                }
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

    /// Parses `"A-B"`, where either bound may be absent. A lone number keeps no line, and any
    /// other text keeps them all.
    fn parse(range: &str) -> Self {
        let range = range.trim_matches('"');
        let Some((start, end)) = range.split_once('-') else {
            let end = if range.parse::<usize>().is_ok() {
                1
            } else {
                usize::MAX
            };
            return Self { start: 1, end };
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
                    ["src", "example", "export", "comment", "verse"]
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

/// Applies `rewrite` to the parts of `line` that are not inline code or verbatim text.
fn outside_code(line: &str, mut rewrite: impl FnMut(&str) -> String) -> String {
    // Where code can end, for each marker: after text and before a break.
    let ends = |marker: char| -> Vec<usize> {
        let closes = |(at, _): &(usize, &str)| {
            let before = line.get(..*at).and_then(|text| text.chars().next_back());
            let after = line.get(*at + marker.len_utf8()..);
            before.is_some_and(|before| !before.is_whitespace())
                && post_ok(after.and_then(|text| text.chars().next()))
        };
        let ends = line.match_indices(marker).filter(closes);
        ends.map(|(at, _)| at).collect()
    };
    let (tildes, equals) = (ends('~'), ends('='));
    let mut rewritten = String::with_capacity(line.len());
    let mut text = 0;
    let mut characters = line.char_indices().peekable();
    let mut previous = None;
    while let Some((open, marker)) = characters.next() {
        let inner = open + marker.len_utf8();
        let opens = is_verbatim_marker(marker)
            && pre_ok(previous)
            && characters
                .peek()
                .is_some_and(|(_, next)| !next.is_whitespace());
        previous = Some(marker);
        let ends = if marker == '~' { &tildes } else { &equals };
        // The first end that leaves text between the two markers.
        let end = opens.then(|| ends.get(ends.partition_point(|at| *at <= inner)));
        if let Some(close) = end.flatten().map(|at| at + marker.len_utf8()) {
            rewritten.push_str(&rewrite(line.get(text..open).unwrap_or("")));
            rewritten.push_str(line.get(open..close).unwrap_or(""));
            text = close;
            while characters.next_if(|(index, _)| *index < close).is_some() {}
        }
    }
    rewritten + &rewrite(line.get(text..).unwrap_or(""))
}

/// Puts `directory` before the relative file target of each bracket link and of each angle link
/// to a file in `text`.
fn rebase_links(text: &str, directory: &str) -> String {
    let brackets = rebase_targets(text, "[[", ']', directory);
    rebase_targets(&brackets, "<file:", '>', directory)
}

/// Rebases the target of each link that starts with `opener` and ends at `close`.
fn rebase_targets(text: &str, opener: &str, close: char, directory: &str) -> String {
    // The scheme in the opener of an angle link is part of the target.
    let lead = opener.trim_end_matches("file:").len();
    let mut rebased = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(opener) {
        let (before, link) = rest.split_at(start + lead);
        rebased.push_str(before);
        let (target, after) = link.split_at(link.find(close).unwrap_or(link.len()));
        let (scheme, path) = match target.strip_prefix("file:") {
            Some(path) => ("file:", path),
            None => ("", target),
        };
        let (path, search) = path.split_at(path.find("::").unwrap_or(path.len()));
        let relative = if scheme.is_empty() {
            path.starts_with("./") || path.starts_with("../")
        } else {
            !path.is_empty() && !path.starts_with(['/', '~']) && path.get(1..2) != Some(":")
        };
        if relative {
            let joined = join(directory, path);
            // A target without a scheme names a file only when it starts with a dot or a slash.
            let dot = if scheme.is_empty() && !joined.starts_with(['.', '/']) {
                "./"
            } else {
                ""
            };
            rebased.extend([scheme, dot, &joined, search]);
        } else {
            rebased.push_str(target);
        }
        rest = after;
    }
    rebased + rest
}

/// The path `path` under `directory`, with each `.` and `..` part resolved by name.
fn join(directory: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in directory.split('/').chain(path.split('/')) {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let root = if directory.starts_with('/') { "/" } else { "" };
    format!("{root}{}", parts.join("/"))
}

/// Puts `prefix` before each of `labels` that a footnote reference or definition in `line` names.
fn relabel(line: &str, labels: &BTreeMap<String, String>, prefix: &str) -> String {
    let mut relabeled = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("[fn:") {
        let (before, after) = rest.split_at(start + "[fn:".len());
        relabeled.push_str(before);
        let end = after.find(|character| !is_footnote_label_char(character));
        let (label, tail) = after.split_at(end.unwrap_or(after.len()));
        if tail.starts_with(']') && labels.contains_key(label) {
            relabeled.push_str(prefix);
        }
        rest = after;
    }
    relabeled + rest
}

fn invalid(message: String) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

/// Charges `bytes` of generated text to the remaining `work`.
fn spend(work: &mut usize, bytes: usize) -> Result<()> {
    *work = work.checked_sub(bytes).ok_or(Error::OutputTooLarge)?;
    Ok(())
}

/// Moves every headline of an included file by the same number of levels, so its shallowest
/// headline sits at `minimum_level`. The stars it adds are charged to `work`.
fn shift_headlines(
    lines: &mut [String],
    minimum_level: Option<usize>,
    work: &mut usize,
) -> Result<()> {
    let Some(minimum_level) = minimum_level else {
        return Ok(());
    };
    let mut blocks = BlockTracker::default();
    let headlines: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !blocks.holds(line))
        .filter_map(|(index, line)| Some((index, headline_level(line)?)))
        .collect();
    let Some(shallowest) = headlines.iter().map(|(_, level)| *level).min() else {
        return Ok(());
    };
    let deeper = minimum_level.saturating_sub(shallowest);
    spend(work, deeper.saturating_mul(headlines.len()))?;
    for (index, _) in headlines {
        let Some(line) = lines.get_mut(index) else {
            continue;
        };
        if deeper > 0 {
            line.insert_str(0, &"*".repeat(deeper));
        } else {
            line.drain(..shallowest - minimum_level);
        }
    }
    Ok(())
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
        assert_eq!(select("\"x\""), "one two three");
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
            let mut work = usize::MAX;
            shift_headlines(&mut lines, Some(minimum_level), &mut work).unwrap();
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

    #[test]
    fn a_relative_file_target_moves_below_the_directory_of_its_file() {
        let rebase = |links| rebase_links(links, "sub");
        let links = "[[file:a.png]] [[./b.png][b]] [[file:../up.png]] <file:n.org::*H>";
        let rebased = "[[file:sub/a.png]] [[./sub/b.png][b]] [[file:up.png]] <file:sub/n.org::*H>";
        assert_eq!(rebase(links), rebased);
        let fixed = "[[file:/a.png]] [[file:~/a.png]] [[https://a.org]] [[a.png]] <./a.png>";
        assert_eq!(rebase(fixed), fixed);
    }

    #[test]
    fn inline_code_is_not_rewritten_and_is_found_in_linear_time() {
        let upper = |line: &str| outside_code(line, str::to_uppercase);
        assert_eq!(upper("a ~b~ c =d e= f"), "A ~b~ C =d e= F");
        assert_eq!(upper("a~b~ c = d= ~e"), "A~B~ C = D= ~E");
        let unclosed = "~a ".repeat(200_000);
        assert_eq!(upper(&unclosed), unclosed.to_uppercase());
    }

    #[test]
    fn a_minimum_level_beyond_the_budget_fails() {
        let mut lines = vec!["* one".to_owned()];
        let mut work = 1000;
        assert!(shift_headlines(&mut lines, Some(usize::MAX), &mut work).is_err());
    }
}
