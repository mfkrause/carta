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
