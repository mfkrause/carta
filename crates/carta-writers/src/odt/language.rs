//! Language fields retained in automatic text-style keys.

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Language {
    pub(super) code: String,
    script: Option<String>,
    pub(super) country: Option<String>,
    subtags: Vec<String>,
}

impl Language {
    pub(super) fn parse(value: &str) -> Option<Self> {
        let end = value
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            .count();
        let normalized = value.get(..end)?.replace('_', "-").to_ascii_lowercase();
        let mut parts = normalized.split('-').peekable();
        let primary = parts.next()?;
        if !(2..=8).contains(&primary.len()) || !letters(primary) {
            return None;
        }
        let mut code = primary.to_string();
        if primary.len() <= 3 {
            for _ in 0..3 {
                let Some(part) = parts.next_if(|part| part.len() == 3 && letters(part)) else {
                    break;
                };
                code.push('-');
                code.push_str(part);
            }
        }
        let script = parts
            .next_if(|part| part.len() == 4 && letters(part))
            .map(str::to_string);
        let country = parts
            .next_if(|part| {
                (part.len() == 2 && letters(part))
                    || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit()))
            })
            .map(str::to_ascii_uppercase);
        let mut subtags = Vec::new();
        while let Some(part) = parts.next_if(|part| {
            (5..=8).contains(&part.len())
                || (part.len() == 4 && part.as_bytes().first().is_some_and(u8::is_ascii_digit))
        }) {
            subtags.push(part.to_string());
        }
        while parts
            .peek()
            .is_some_and(|part| part.len() == 1 && *part != "x")
        {
            let mut extension = parts.clone();
            let mut values = vec![extension.next()?.to_string()];
            while let Some(part) = extension.next_if(|part| (2..=8).contains(&part.len())) {
                values.push(part.to_string());
            }
            if values.len() == 1 {
                break;
            }
            subtags.extend(values);
            parts = extension;
        }
        if parts.peek() == Some(&"x") {
            parts.next();
            let mut values = vec!["x".to_string()];
            while let Some(part) = parts.next_if(|part| (1..=8).contains(&part.len())) {
                values.push(part.to_string());
            }
            if values.len() > 1 {
                subtags.extend(values);
            }
        }
        Some(Self {
            code,
            script,
            country,
            subtags,
        })
    }

    pub(super) fn clone_size(&self) -> usize {
        self.code
            .len()
            .saturating_add(self.script.as_ref().map_or(0, String::len))
            .saturating_add(self.country.as_ref().map_or(0, String::len))
            .saturating_add(
                self.subtags
                    .len()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(self.subtags.iter().map(String::len).sum::<usize>())
    }
}

fn letters(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_fields_keep_valid_prefixes() {
        for (tag, code, country) in [
            ("FR_ca", "fr", Some("CA")),
            ("zh-cmn-Hant-TW", "zh-cmn", Some("TW")),
            ("en-u-ca-gregory", "en", None),
            ("en--US", "en", None),
            ("en-US-extra-GB", "en", Some("US")),
            ("en-123", "en", Some("123")),
        ] {
            let language = Language::parse(tag).expect("language");
            assert_eq!(language.code, code, "{tag}");
            assert_eq!(language.country.as_deref(), country, "{tag}");
        }
        for tag in ["a", "x-private", "123", " en-US", "en123"] {
            assert!(Language::parse(tag).is_none(), "{tag}");
        }
    }

    #[test]
    fn style_keys_retain_scripts_variants_and_extensions() {
        for (left, right) in [
            ("zh-Hans-CN", "zh-Hant-CN"),
            ("en-GB-1901", "en-GB-1902"),
            ("en-u-ca-gregory", "en-u-ca-buddhist"),
        ] {
            assert!(Language::parse(left) != Language::parse(right));
        }
        assert!(Language::parse("FR_ca") == Language::parse("fr-CA"));
        assert!(Language::parse("en-US") < Language::parse("en-Latn-BE"));
    }
}
