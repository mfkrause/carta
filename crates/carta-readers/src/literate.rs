//! Parse literal code sections in prose documents.

use carta_ast::Attr;

pub(crate) fn code_attr() -> Attr {
    Attr {
        classes: vec!["haskell".into(), "literate".into()],
        ..Attr::default()
    }
}

#[cfg(any(feature = "commonmark", feature = "rst"))]
pub(crate) fn bird_tracks(lines: impl Iterator<Item = impl AsRef<str>>) -> Option<(String, usize)> {
    let mut body = Vec::new();
    for line in lines {
        let line = line.as_ref();
        if let Some(content) = line.strip_prefix('>') {
            body.push(content.to_owned());
        } else if line.trim().is_empty() {
            break;
        } else {
            return None;
        }
    }
    if body.is_empty() {
        return None;
    }
    let padded = body
        .iter()
        .all(|line| line.is_empty() || line.starts_with(' '));
    if padded {
        for line in &mut body {
            if let Some(content) = line.strip_prefix(' ') {
                *line = content.to_owned();
            }
        }
    }
    Some((body.join("\n"), body.len()))
}

#[cfg(all(test, any(feature = "commonmark", feature = "rst")))]
mod tests {
    use super::bird_tracks;

    #[test]
    fn tracks_keep_empty_lines_and_remove_only_shared_padding() {
        assert_eq!(
            bird_tracks([">", "> a", ">", "", "after"].into_iter()),
            Some(("\na\n".into(), 3))
        );
        assert_eq!(
            bird_tracks(["> a", ">b"].into_iter()),
            Some((" a\nb".into(), 2))
        );
        assert_eq!(
            bird_tracks([">  a", "> b"].into_iter()),
            Some((" a\nb".into(), 2))
        );
        assert_eq!(bird_tracks(["> a", "prose"].into_iter()), None);
    }
}
