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
                '<' if at(chars, position + 1) == Some('/') => index.closes.push(position),
                '\'' => std::mem::swap(&mut clear, &mut single),
                '"' => std::mem::swap(&mut clear, &mut double),
                _ => {}
            }
            if let Some(slot) = index.tag_ends.get_mut(position) {
                *slot = clear;
            }
        }
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
        index
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
