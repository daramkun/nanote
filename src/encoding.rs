//! Text encodings: detection, decoding and encoding of file contents.

use std::path::Path;

use crate::i18n::tr;

pub const ENCODINGS: &[&str] = &[
    "UTF-8",
    "UTF-16LE",
    "UTF-16BE",
    "UTF-32LE",
    "UTF-32BE",
    "EUC-KR",
    "Shift_JIS",
    "EUC-JP",
    "ISO-2022-JP",
    "GBK",
    "gb18030",
    "Big5",
    "windows-1250",
    "windows-1251",
    "windows-1252",
    "windows-1253",
    "windows-1254",
    "windows-1255",
    "windows-1256",
    "windows-1257",
    "windows-1258",
    "windows-874",
    "ISO-8859-2",
    "ISO-8859-3",
    "ISO-8859-4",
    "ISO-8859-5",
    "ISO-8859-6",
    "ISO-8859-7",
    "ISO-8859-8",
    "ISO-8859-10",
    "ISO-8859-13",
    "ISO-8859-14",
    "ISO-8859-15",
    "ISO-8859-16",
    "KOI8-R",
    "KOI8-U",
    "IBM866",
    "macintosh",
];

pub struct Loaded {
    pub text: String,
    pub encoding: String,
    pub bom: bool,
    pub crlf: bool,
}

/// BOM bytes for a Unicode encoding, if it has one.
pub fn bom_bytes(encoding: &str) -> Option<&'static [u8]> {
    match encoding {
        "UTF-8" => Some(&[0xEF, 0xBB, 0xBF]),
        "UTF-16LE" => Some(&[0xFF, 0xFE]),
        "UTF-16BE" => Some(&[0xFE, 0xFF]),
        "UTF-32LE" => Some(&[0xFF, 0xFE, 0, 0]),
        "UTF-32BE" => Some(&[0, 0, 0xFE, 0xFF]),
        _ => None,
    }
}

pub fn sniff_encoding(bytes: &[u8]) -> String {
    // UTF-32 LE must be tested before UTF-16 LE (same first two bytes).
    for enc in ["UTF-8", "UTF-32LE", "UTF-32BE", "UTF-16LE", "UTF-16BE"] {
        if bytes.starts_with(bom_bytes(enc).unwrap()) {
            return enc.into();
        }
    }
    if std::str::from_utf8(bytes).is_ok() {
        return "UTF-8".into();
    }
    // BOM-less UTF-16: lots of NUL bytes in either the odd or even positions.
    if bytes.len() >= 4 {
        let n = bytes.len() / 2;
        let even_nul = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd_nul = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        if odd_nul * 10 > n * 3 && even_nul * 10 < n {
            return "UTF-16LE".into();
        }
        if even_nul * 10 > n * 3 && odd_nul * 10 < n {
            return "UTF-16BE".into();
        }
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    det.feed(bytes, true);
    det.guess(None, chardetng::Utf8Detection::Deny)
        .name()
        .to_owned()
}

pub fn decode_bytes(bytes: &[u8], encoding: &str) -> (String, bool) {
    let bom = bom_bytes(encoding).filter(|b| bytes.starts_with(b));
    let body = &bytes[bom.map_or(0, |b| b.len())..];
    let text = match encoding {
        "UTF-8" => String::from_utf8_lossy(body).into_owned(),
        "UTF-16LE" | "UTF-16BE" => {
            let le = encoding == "UTF-16LE";
            let units: Vec<u16> = body
                .chunks_exact(2)
                .map(|c| {
                    if le {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        }
        "UTF-32LE" | "UTF-32BE" => {
            let le = encoding == "UTF-32LE";
            body.chunks_exact(4)
                .map(|c| {
                    let a = [c[0], c[1], c[2], c[3]];
                    let v = if le {
                        u32::from_le_bytes(a)
                    } else {
                        u32::from_be_bytes(a)
                    };
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                })
                .collect()
        }
        other => {
            let enc =
                encoding_rs::Encoding::for_label(other.as_bytes()).unwrap_or(encoding_rs::UTF_8);
            enc.decode_without_bom_handling(body).0.into_owned()
        }
    };
    (text, bom.is_some())
}

pub fn read_file(path: &Path, forced: Option<&str>) -> std::io::Result<Loaded> {
    let bytes = std::fs::read(path)?;
    let encoding = forced
        .map(str::to_owned)
        .unwrap_or_else(|| sniff_encoding(&bytes));
    let (raw, bom) = decode_bytes(&bytes, &encoding);
    let crlf = raw.contains("\r\n");
    let text = if crlf { raw.replace("\r\n", "\n") } else { raw };
    Ok(Loaded {
        text,
        encoding,
        bom,
        crlf,
    })
}

pub fn encode_text(text: &str, encoding: &str, bom: bool, crlf: bool) -> Result<Vec<u8>, String> {
    let text = if crlf {
        std::borrow::Cow::Owned(text.replace('\n', "\r\n"))
    } else {
        text.into()
    };
    let mut out: Vec<u8> = Vec::new();
    if bom {
        if let Some(b) = bom_bytes(encoding) {
            out.extend_from_slice(b);
        }
    }
    match encoding {
        "UTF-8" => out.extend_from_slice(text.as_bytes()),
        "UTF-16LE" => out.extend(text.encode_utf16().flat_map(u16::to_le_bytes)),
        "UTF-16BE" => out.extend(text.encode_utf16().flat_map(u16::to_be_bytes)),
        "UTF-32LE" => out.extend(text.chars().flat_map(|c| (c as u32).to_le_bytes())),
        "UTF-32BE" => out.extend(text.chars().flat_map(|c| (c as u32).to_be_bytes())),
        other => {
            let enc = encoding_rs::Encoding::for_label(other.as_bytes())
                .ok_or_else(|| format!("Unknown encoding: {other}"))?;
            let (bytes, _, unmappable) = enc.encode(&text);
            if unmappable {
                return Err(format!(
                    "{} {other}{}",
                    tr("Some characters cannot be represented in"),
                    tr("; file not saved")
                ));
            }
            out.extend_from_slice(&bytes);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_round_trip() {
        let sample = "Hello 안녕하세요 こんにちは\nline2";
        for enc in ["UTF-8", "UTF-16LE", "UTF-16BE", "UTF-32LE", "UTF-32BE"] {
            for bom in [false, true] {
                let bytes = encode_text(sample, enc, bom, false).unwrap();
                if bom {
                    assert!(bytes.starts_with(bom_bytes(enc).unwrap()));
                    assert_eq!(sniff_encoding(&bytes), enc);
                }
                let (text, had_bom) = decode_bytes(&bytes, enc);
                assert_eq!((text.as_str(), had_bom), (sample, bom), "{enc} bom={bom}");
            }
        }
        let kr = encode_text("안녕하세요 한글", "EUC-KR", false, false).unwrap();
        assert_eq!(decode_bytes(&kr, "EUC-KR").0, "안녕하세요 한글");
        let jp = encode_text("こんにちは日本語", "Shift_JIS", false, false).unwrap();
        assert_eq!(decode_bytes(&jp, "Shift_JIS").0, "こんにちは日本語");
        assert!(encode_text("한글", "windows-1252", false, false).is_err());
        assert_eq!(
            encode_text("a\nb", "UTF-8", false, true).unwrap(),
            b"a\r\nb"
        );
    }
}
