//! Shared positions for scans through nested source ranges.

use std::collections::BTreeMap;

use super::{ScanBounds, at, line_end};

type Closing = Option<(usize, usize)>;

pub(super) struct ScanIndex {
    address: usize,
    length: usize,
    lines: Vec<usize>,
    angles: Vec<usize>,
    closes: Vec<usize>,
    tag_ends: Vec<usize>,
    tables: Vec<(usize, Closing)>,
    nested_tables: Vec<(usize, Closing)>,
    carriages: Vec<usize>,
    tags: BTreeMap<String, Vec<(usize, Closing)>>,
}

impl ScanIndex {
    pub(super) fn new(chars: &[char]) -> Self {
        let mut index = Self {
            address: chars.as_ptr() as usize,
            length: chars.len(),
            lines: Vec::new(),
            angles: Vec::new(),
            closes: Vec::new(),
            tag_ends: vec![chars.len(); chars.len()],
            tags: BTreeMap::new(),
            tables: Vec::new(),
            nested_tables: Vec::new(),
            carriages: Vec::new(),
        };
        // Resolve quoted endings once so malformed openers do not rescan the suffix.
        let (mut clear, mut single, mut double) = (chars.len(), chars.len(), chars.len());
        for (position, ch) in chars.iter().enumerate().rev() {
            match ch {
                '>' => {
                    clear = position;
                    index.angles.push(position);
                }
                '\n' => index.lines.push(position),
                '\r' => index.carriages.push(position),
                '<' if at(chars, position + 1) == Some('/') => index.closes.push(position),
                '\'' => std::mem::swap(&mut clear, &mut single),
                '"' => std::mem::swap(&mut clear, &mut double),
                _ => {}
            }
            if let Some(slot) = index.tag_ends.get_mut(position) {
                *slot = clear;
            }
        }
        index.carriages.reverse();
        index.angles.reverse();
        index.lines.reverse();
        index.closes.reverse();
        let mut list_after = 0;
        let mut events = BTreeMap::<String, Vec<(usize, bool)>>::new();
        for (position, ch) in chars.iter().enumerate() {
            if *ch == '<' {
                let closing = at(chars, position + 1) == Some('/');
                let start = position + if closing { 2 } else { 1 };
                let name: String = chars
                    .get(start..)
                    .unwrap_or(&[])
                    .iter()
                    .take_while(|ch| ch.is_ascii_alphanumeric())
                    .map(char::to_ascii_lowercase)
                    .collect();
                if name.is_empty()
                    || !at(chars, start + name.len())
                        .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '>' | '/'))
                {
                    continue;
                }
                if position >= list_after && matches!(name.as_str(), "ul" | "ol") {
                    let end = if closing {
                        index
                            .angles
                            .get(index.angles.partition_point(|&angle| angle < position))
                            .copied()
                    } else {
                        index.tag_end(start + name.len(), chars.len())
                    };
                    if closing
                        || end.is_some_and(|end| at(chars, end.saturating_sub(1)) != Some('/'))
                    {
                        events
                            .entry(String::new())
                            .or_default()
                            .push((position, closing));
                    }
                    if let Some(end) = end {
                        list_after = end + 1;
                    }
                }
                events.entry(name).or_default().push((position, closing));
            }
        }
        for (name, events) in events {
            let mut pending = Vec::new();
            let mut entries = Vec::with_capacity(events.len());
            for (position, closing) in events.into_iter().rev() {
                if closing {
                    let angle = index
                        .angles
                        .get(index.angles.partition_point(|&angle| angle < position))
                        .copied();
                    pending.push(angle.map(|angle| (position, angle + 1)));
                } else {
                    pending.pop();
                }
                entries.push((position, pending.last().copied().flatten()));
            }
            entries.reverse();
            index.tags.insert(name, entries);
        }
        index.tables = index.table_positions(chars, false);
        index.nested_tables = index.table_positions(chars, true);
        index
    }

    fn table_positions(&self, chars: &[char], nested: bool) -> Vec<(usize, Closing)> {
        let mut events = Vec::new();
        let mut line = 0;
        while line < chars.len() {
            let end = self.line_end(chars, line);
            let mut content = line;
            while content < end
                && at(chars, content).is_some_and(|ch| {
                    if nested {
                        ch.is_whitespace()
                    } else {
                        matches!(ch, ' ' | '\t')
                    }
                })
            {
                content += 1;
            }
            if at(chars, content) == Some('{') && at(chars, content + 1) == Some('|') {
                events.push((content, false));
            } else if at(chars, content) == Some('|') && at(chars, content + 1) == Some('}') {
                events.push((content, true));
            }
            line = end + 1;
        }
        let mut pending = Vec::new();
        let mut positions = Vec::with_capacity(events.len());
        for (position, closing) in events.into_iter().rev() {
            if closing {
                pending.push((position, position + 2));
            } else {
                pending.pop();
            }
            positions.push((position, pending.last().copied()));
        }
        positions.reverse();
        positions
    }

    fn range(&self, chars: &[char]) -> Option<(usize, usize)> {
        let bytes = (chars.as_ptr() as usize).checked_sub(self.address)?;
        if !bytes.is_multiple_of(std::mem::size_of::<char>()) {
            return None;
        }
        let start = bytes / std::mem::size_of::<char>();
        let end = start.checked_add(chars.len())?;
        (end <= self.length).then_some((start, end))
    }

    pub(super) fn bounds<'a>(&'a self, chars: &[char]) -> ScanBounds<'a> {
        let Some((start, end)) = self.range(chars) else {
            return ScanBounds::of(chars);
        };
        let last = |positions: &[usize]| {
            positions
                .get(..positions.partition_point(|&p| p < end))
                .and_then(|positions| positions.last())
                .copied()
                .filter(|&position| position >= start)
                .map(|position| position - start)
        };
        ScanBounds {
            index: Some((self, start, end)),
            last_gt: last(&self.angles),
            last_close: last(&self.closes),
        }
    }

    pub(super) fn line_end(&self, chars: &[char], position: usize) -> usize {
        let Some((start, end)) = self.range(chars) else {
            return line_end(chars, position);
        };
        self.lines
            .get(self.lines.partition_point(|&line| line < start + position))
            .copied()
            .unwrap_or(end)
            .min(end)
            - start
    }

    pub(super) fn table_end(
        &self,
        chars: &[char],
        position: usize,
        nested: bool,
    ) -> Option<(usize, usize)> {
        let (start, end) = self.range(chars)?;
        let tables = if nested {
            &self.nested_tables
        } else {
            &self.tables
        };
        let line_end = start + self.line_end(chars, position);
        let (close, after) = tables
            .get(tables.partition_point(|&(position, _)| position <= line_end))
            .and_then(|&(_, closing)| closing)
            .unwrap_or((end, end));
        Some((close.min(end) - start, after.min(end) - start))
    }

    pub(super) fn has_carriage(&self, chars: &[char]) -> bool {
        let Some((start, end)) = self.range(chars) else {
            return chars.contains(&'\r');
        };
        self.carriages
            .get(self.carriages.partition_point(|&position| position < start))
            .is_some_and(|&position| position < end)
    }

    pub(super) fn tag_end(&self, start: usize, end: usize) -> Option<usize> {
        self.tag_ends
            .get(start)
            .copied()
            .filter(|&position| position < end)
    }

    pub(super) fn close(&self, start: usize, end: usize, name: &str) -> Closing {
        let positions = self.tags.get(name)?;
        let (_, closing) =
            positions.get(positions.partition_point(|&(position, _)| position < start))?;
        closing.filter(|&(_, after)| after <= end)
    }
}
