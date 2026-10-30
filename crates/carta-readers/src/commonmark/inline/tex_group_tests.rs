use super::TexGroups;

fn group_end(text: &[u8], start: usize, comments: bool) -> Option<usize> {
    let opening = *text.get(start)?;
    let mut braces = usize::from(opening == b'{');
    let mut brackets = usize::from(opening == b'[');
    let mut position = start + 1;
    while let Some(&character) = text.get(position) {
        position += 1;
        match character {
            b'\\' => position += 1,
            b'%' if comments => {
                while text.get(position).is_some_and(|&c| c != b'\n') {
                    position += 1;
                }
            }
            b'{' => braces += 1,
            b'}' => {
                if braces == 0 {
                    return None;
                }
                braces -= 1;
                if opening == b'{' && braces == 0 {
                    return Some(position);
                }
            }
            b'[' if braces == 0 => brackets += 1,
            b']' if braces == 0 => {
                brackets -= 1;
                if brackets == 0 {
                    return Some(position);
                }
            }
            _ => {}
        }
    }
    None
}

fn body_start(text: &[u8], mut position: usize) -> Option<usize> {
    while let Some(&character) = text.get(position) {
        match character {
            b'\\' => position += 2,
            b'%' => {
                while text.get(position).is_some_and(|&c| c != b'\n') {
                    position += 1;
                }
            }
            b'{' => return Some(position),
            _ => position += 1,
        }
    }
    None
}

#[test]
fn cached_groups_match_independent_forward_scans() {
    let alphabet = b"{}[]%\n\\ab";
    let mut state = 7u64;
    for _ in 0..5000 {
        let mut text = String::new();
        for _ in 0..state % 32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            text.push(char::from(
                *alphabet.get(usize::try_from(state % 9).unwrap()).unwrap(),
            ));
        }
        state = state.wrapping_add(1);
        for comments in [false, true] {
            let groups = TexGroups::new(&text, comments);
            let mut escaped = false;
            for (position, character) in text.bytes().enumerate() {
                if escaped {
                    escaped = false;
                    continue;
                }
                escaped = character == b'\\';
                if matches!(character, b'{' | b'[') {
                    assert_eq!(
                        groups.ends.get(&position).copied(),
                        group_end(text.as_bytes(), position, comments),
                        "{text:?}, {position}, comments={comments}"
                    );
                }
                if comments {
                    assert_eq!(
                        groups
                            .bodies
                            .range(position..)
                            .next()
                            .and_then(|(_, body)| *body),
                        body_start(text.as_bytes(), position),
                        "{text:?}, {position}"
                    );
                }
            }
        }
    }
}
