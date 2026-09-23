//! Explicit, strict decoding. A successful legacy decode is not evidence that
//! the encoding was correct: callers must show a preview before importing.
use encoding_rs::Encoding;

pub(crate) const ENCODINGS: &[(&str, &Encoding)] = &[
    ("GB18030 / GBK / GB2312", encoding_rs::GB18030),
    ("Big5", encoding_rs::BIG5),
    ("Shift-JIS", encoding_rs::SHIFT_JIS),
    ("EUC-JP", encoding_rs::EUC_JP),
    ("ISO-2022-JP", encoding_rs::ISO_2022_JP),
    ("EUC-KR", encoding_rs::EUC_KR),
    ("Windows-1252", encoding_rs::WINDOWS_1252),
    ("Mac Roman", encoding_rs::MACINTOSH),
];

pub(crate) fn decode(bytes: &[u8], encoding: &'static Encoding) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err("NUL bytes found: this may be a binary or UTF-16 file.".into());
    }
    encoding.decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| text.into_owned())
        .ok_or_else(|| format!("Invalid {} bytes. No replacement characters were inserted; try another encoding or repair the mixed input separately.", encoding.name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_manual_fixtures_match_utf8_goldens() {
        for (bytes, encoding, expected) in [
            (
                &include_bytes!("../manual-tests/encodings/gb18030.tex")[..],
                encoding_rs::GB18030,
                include_str!("../manual-tests/encodings/gb18030.expected.txt"),
            ),
            (
                &include_bytes!("../manual-tests/encodings/big5.tex")[..],
                encoding_rs::BIG5,
                include_str!("../manual-tests/encodings/big5.expected.txt"),
            ),
            (
                &include_bytes!("../manual-tests/encodings/shift-jis.tex")[..],
                encoding_rs::SHIFT_JIS,
                include_str!("../manual-tests/encodings/shift-jis.expected.txt"),
            ),
            (
                &include_bytes!("../manual-tests/encodings/windows-1252.tex")[..],
                encoding_rs::WINDOWS_1252,
                include_str!("../manual-tests/encodings/windows-1252.expected.txt"),
            ),
        ] {
            assert_eq!(decode(bytes, encoding).unwrap(), expected);
            let (round_trip, _, errors) = encoding.encode(expected);
            assert!(!errors);
            assert_eq!(round_trip, bytes);
        }
    }

    #[test]
    fn verified_legacy_source_excerpts() {
        // Byte excerpts verified against CTAN CJK 4.8.5 examples and arXiv
        // 0709.2497/comment_apl.tex. See docs/text-encoding.md for provenance.
        for (bytes, encoding, expected) in [
            (
                &b"\xb1\xbe\xb3\xa3\xce\xca\xce\xca\xb4\xf0\xbc\xaf"[..],
                encoding_rs::GB18030,
                "本常问问答集",
            ),
            (
                &b"\xa5\xbb\xb1\x60\xb0\xdd\xb0\xdd\xb5\xaa\xb6\xb0"[..],
                encoding_rs::BIG5,
                "本常問問答集",
            ),
            (
                &b"\x82\xb1\x82\xcc~FAQ~\x83\x8a\x83\x58\x83\x67"[..],
                encoding_rs::SHIFT_JIS,
                "この~FAQ~リスト",
            ),
            (&b"141\x96--147"[..], encoding_rs::WINDOWS_1252, "141–--147"),
        ] {
            assert_eq!(decode(bytes, encoding).unwrap(), expected);
        }
    }
    #[test]
    fn rejects_incomplete_and_binary_input_without_lossy_replacement() {
        assert!(decode(b"\x81", encoding_rs::GB18030).is_err());
        assert!(decode(b"abc\0def", encoding_rs::WINDOWS_1252).is_err());
        assert!(decode(b"\x82", encoding_rs::SHIFT_JIS).is_err());
    }
    #[test]
    fn valid_utf8_is_not_a_reason_to_convert_it() {
        let original = "中文 — café";
        assert!(std::str::from_utf8(original.as_bytes()).is_ok());
        let mut mixed = original.as_bytes().to_vec();
        mixed.push(0xed);
        assert!(std::str::from_utf8(&mixed).is_err());
        // Mixed data may decode without errors under a wrong encoding. There
        // is deliberately no detector or automatic whole-file fallback here.
        assert_ne!(decode(&mixed, encoding_rs::WINDOWS_1252).unwrap(), original);
    }
}
