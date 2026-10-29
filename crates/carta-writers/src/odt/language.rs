//! Language fields for automatic text styles and caption labels.

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

pub(super) fn caption_labels(value: &str) -> (&'static str, &'static str) {
    let Some(language) = Language::parse(value) else {
        return ("Figure", "Table");
    };
    match (language.code.as_str(), language.script.as_deref()) {
        ("zh", Some("hans")) => ("图", "表"),
        ("zh", Some("hant")) => ("圖", "表"),
        ("sr", Some("latn")) => ("Slika", "Tabela"),
        (code, _) => CAPTION_LABELS
            .iter()
            .find_map(|(language, labels)| (*language == code).then_some(*labels))
            .unwrap_or(("", "")),
    }
}

const CAPTION_LABELS: &[(&str, (&str, &str))] = &[
    ("", ("Figure", "Table")),
    ("a", ("Figure", "Table")),
    ("af", ("Figuur", "Tabel")),
    ("alt", ("Јурук", "")),
    ("am", ("ሥዕል", "ሰንጠረዥ")),
    ("ar", ("شكل", "جدول")),
    ("as", ("চিত্ৰ", "তালিকা")),
    ("ast", ("Figura", "Tabla")),
    ("az", ("Şəkil", "Cədvəl")),
    ("be", ("Рыс.", "Табліца")),
    ("bg", ("Фигура", "Таблица")),
    ("bn", ("ছবি/নকশা", "সারনী")),
    ("bo", ("པར་རིས་", "རེའུ་མིག་")),
    ("br", ("Figurenn", "Taolenn")),
    ("bs", ("Slika", "Tabela")),
    ("bua", ("Зураг", "Хүсэнэгт")),
    ("ca", ("Figura", "Taula")),
    ("cs", ("Obrázek", "Tabulka")),
    ("cu", ("Рис.", "Таблица")),
    ("cy", ("Darlun", "Taflen")),
    ("cz", ("Obrázek", "Tabulka")),
    ("da", ("Figur", "Tabel")),
    ("de", ("Abbildung", "Tabelle")),
    ("dsb", ("Wobraz", "Tabulka")),
    ("el", ("Σχήμα", "Πίνακας")),
    ("en", ("Figure", "Table")),
    ("eo", ("Figuro", "Tabelo")),
    ("es", ("Figura", "Cuadro")),
    ("et", ("Joonis", "Tabel")),
    ("eu", ("Irudia", "Taula")),
    ("fa", ("شكل", "جدول")),
    ("fi", ("Kuva", "Taulukko")),
    ("fil", ("Laraw", "Talay")),
    ("fr", ("Figure", "Tableau")),
    ("fur", ("Figure", "Tabele")),
    ("ga", ("Léaráid", "Tábla")),
    ("gd", ("Dealbh", "Clàr")),
    ("gl", ("Figura", "Cadro")),
    ("grc", ("Σχῆμα", "Π\u{1f77}ναξ")),
    ("gu", ("", "ટેબલ")),
    ("ha", ("Addadi", "tabili")),
    ("he", ("איור", "טבלה")),
    ("hi", ("चित्र", "तालिका")),
    ("hr", ("Slika", "Tablica")),
    ("hsb", ("Wobraz", "Tabulka")),
    ("hu", ("ábra", "táblázat")),
    ("hy", ("Նկար", "Աղյուսակ")),
    ("ia", ("Figura", "Tabella")),
    ("id", ("Gambar", "Tabel")),
    ("is", ("Mynd", "Tafla")),
    ("it", ("Figura", "Tabella")),
    ("ja", ("図", "表")),
    ("ka", ("სურ.", "ცხრ.")),
    ("km", ("រូប", "តារាង")),
    ("kn", ("ಚಿತ್ರ", "ಕೋಷ್ಟಕ")),
    ("ko", ("그림", "표")),
    ("la", ("Descriptio", "Tabula")),
    ("lb", ("Bild", "Tabell")),
    ("lo", ("ຮູບທີ", "ຕາຕະລາງທີ")),
    ("lt", ("pav.", "lentelė")),
    ("lv", ("Att.", "Tabula")),
    ("mk", ("Слика", "Табела")),
    ("ml", ("ചിത്രം", "പട്ടിക")),
    ("mn", ("Зураг", "Хүснэгт")),
    ("mr", ("आकृती", "कोष्टक")),
    ("ms", ("Gambar", "Jadual")),
    ("nb", ("Figur", "Tabell")),
    ("nko", ("ߢߊ", "ߦߌ߬ߘߊ߬ߥߟߊ")),
    ("nl", ("Figuur", "Tabel")),
    ("nn", ("Figur", "Tabell")),
    ("no", ("Figur", "Tabell")),
    ("oc", ("Figura", "Tablèu")),
    ("or", ("ପ୍ରତିଛବି", "ଟେବୁଲ୍")),
    ("pa", ("\u{a36}ਕਲ", "ਲੇਖਾ")),
    ("pl", ("Rysunek", "Tabela")),
    ("pms", ("Figura", "Tabela")),
    ("pt", ("Figura", "Tabela")),
    ("rm", ("Figura", "Tabella")),
    ("ro", ("Figura", "Tabela")),
    ("ru", ("Рис.", "Таблица")),
    ("se", ("Govus", "Tabealla")),
    ("si", ("සටහන", "වගුව")),
    ("sk", ("Obr.", "Tabuľka")),
    ("sl", ("Slika", "Tabela")),
    ("sq", ("Figura", "Tabela")),
    ("sr", ("Слика", "Табела")),
    ("sv", ("Figur", "Tabell")),
    ("ta", ("படம்", "அட்டவணை")),
    ("te", ("ఆకృతి", "పట్టిక")),
    ("th", ("รูปที่", "ตารางที่")),
    ("tk", ("Surat", "Tablisa")),
    ("tr", ("Şekil", "Tablo")),
    ("ua", ("Ілюстрація", "Таблиця")),
    ("ug", ("رەسىم", "جەدۋەل")),
    ("uk", ("Рис.", "Табл.")),
    ("ur", ("شكل", "جدول")),
    ("vi", ("Hình", "Bảng")),
];

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
