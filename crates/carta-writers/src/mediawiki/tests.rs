use super::*;

fn attr(pairs: &[(&str, &str)]) -> Attr {
    Attr {
        attributes: pairs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        ..Attr::default()
    }
}

#[test]
fn image_size_converts_dimensions_to_pixels() {
    assert_eq!(
        image_size(&attr(&[("width", "1in"), ("height", "0.5in")])),
        Some("96x48px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("width", "2in")])),
        Some("192px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("height", "1in")])),
        Some("x96px".to_owned())
    );
    assert_eq!(
        image_size(&attr(&[("width", "120px")])),
        Some("120px".to_owned())
    );
    assert_eq!(image_size(&attr(&[("width", "50%")])), None);
    assert_eq!(image_size(&attr(&[])), None);
}

#[test]
fn plain_runs_on_into_the_next_block() {
    let plain = Block::Plain(Vec::new());
    let para = Block::Para(Vec::new());
    assert_eq!(separator(&plain, &para, false), "\n");
    assert_eq!(separator(&plain, &Block::HorizontalRule, false), "\n\n");
    assert_eq!(separator(&para, &plain, false), "\n\n");
}
