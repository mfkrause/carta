//! Shared fragments for math output.

use std::rc::Rc;

use super::values::Values;

#[derive(Debug, Clone)]
pub(super) struct MathText(Rc<Fragment>);

#[derive(Debug)]
struct Fragment {
    text: String,
    parts: Values<MathText>,
    length: usize,
    characters: usize,
    first: Option<char>,
    last: Option<char>,
    letters: bool,
    control: bool,
    head: Vec<u8>,
}

impl Drop for Fragment {
    fn drop(&mut self) {
        let mut parts = std::mem::take(&mut self.parts);
        while let Some(MathText(part)) = parts.pop() {
            if let Ok(mut part) = Rc::try_unwrap(part) {
                parts.append(&mut part.parts);
            }
        }
    }
}

impl Default for MathText {
    fn default() -> Self {
        Self::from(String::new())
    }
}

impl From<String> for MathText {
    fn from(text: String) -> Self {
        Self(Rc::new(Fragment {
            length: text.len(),
            characters: text.chars().count(),
            first: text.chars().next(),
            last: text.chars().next_back(),
            letters: text.bytes().all(|byte| byte.is_ascii_alphabetic()),
            control: super::ends_control_sequence(&text),
            head: text.bytes().take(6).collect(),
            text,
            parts: Values::default(),
        }))
    }
}

impl From<&str> for MathText {
    fn from(text: &str) -> Self {
        Self::from(text.to_string())
    }
}

impl From<&Self> for MathText {
    fn from(text: &Self) -> Self {
        text.clone()
    }
}

impl MathText {
    pub(super) fn concat(parts: Vec<Self>) -> Self {
        let mut parts: Vec<Self> = parts.into_iter().filter(|part| !part.is_empty()).collect();
        if parts.len() == 1 {
            return parts.pop().unwrap_or_default();
        }
        let mut fragment = Fragment {
            text: String::new(),
            parts: Values::default(),
            length: 0,
            characters: 0,
            first: None,
            last: None,
            letters: true,
            control: false,
            head: Vec::new(),
        };
        for part in &parts {
            let tail = &part.0;
            fragment.control = if tail.letters {
                fragment.last == Some('\\')
                    || fragment.control && fragment.last.is_some_and(|ch| ch.is_ascii_alphabetic())
            } else if tail.characters == 1 && !tail.last.is_some_and(char::is_whitespace) {
                fragment.last == Some('\\')
            } else {
                tail.control
            };
            fragment.length = fragment.length.saturating_add(tail.length);
            fragment.characters = fragment.characters.saturating_add(tail.characters);
            fragment.first = fragment.first.or(tail.first);
            fragment.last = tail.last;
            fragment.letters &= tail.letters;
            let remaining = 6usize.saturating_sub(fragment.head.len());
            fragment.head.extend(tail.head.iter().take(remaining));
        }
        fragment.parts = parts.into();
        Self(Rc::new(fragment))
    }

    pub(super) fn join(parts: impl IntoIterator<Item = Self>, separator: &str) -> Self {
        let mut joined = Vec::new();
        for part in parts {
            if !joined.is_empty() {
                joined.push(Self::from(separator));
            }
            joined.push(part);
        }
        Self::concat(joined)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.length == 0
    }

    pub(super) fn first(&self) -> Option<char> {
        self.0.first
    }

    pub(super) fn last(&self) -> Option<char> {
        self.0.last
    }

    pub(super) fn ends_control(&self) -> bool {
        self.0.control
    }

    pub(super) fn equals(&self, text: &str) -> bool {
        self.0.length == text.len() && self.render() == text
    }

    pub(super) fn trim_end(&self) -> Self {
        carta_core::with_stack(|| {
            if !self.last().is_some_and(char::is_whitespace) {
                return self.clone();
            }
            if self.0.parts.is_empty() {
                return Self::from(self.0.text.trim_end());
            }
            let mut parts = self.0.parts.to_vec();
            while let Some(last) = parts.pop() {
                let trimmed = last.trim_end();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                    break;
                }
            }
            Self::concat(parts)
        })
    }

    pub(super) fn strip_text(&self) -> Self {
        if self.0.head != b"\\text{" || self.last() != Some('}') {
            return self.clone();
        }
        if self
            .0
            .parts
            .first()
            .is_some_and(|part| part.0.text == "\\text{")
            && self.0.parts.last().is_some_and(|part| part.0.text == "}")
        {
            return Self::concat(
                self.0
                    .parts
                    .get(1..self.0.parts.len().saturating_sub(1))
                    .unwrap_or(&[])
                    .to_vec(),
            );
        }
        let text = self.render();
        Self::from(
            text.strip_prefix("\\text{")
                .and_then(|text| text.strip_suffix('}'))
                .unwrap_or(&text),
        )
    }

    pub(super) fn render(&self) -> String {
        let mut text = String::with_capacity(self.0.length);
        let mut pending = vec![self];
        while let Some(part) = pending.pop() {
            text.push_str(&part.0.text);
            pending.extend(part.0.parts.iter().rev());
        }
        text
    }
}
