//! Parsers (agent A): JPEG markers + baseline Huffman decoding to coefficients, PNG chunks, WebP RIFF chunks,
//! HEIF/ISOBMFF boxes, EXIF/XMP/IPTC metadata, and the structure/metadata signals.
//!
//! Every parser is lenient: a damaged or truncated file still yields a [`ParsedImage`] with what could be read,
//! the damaged part recorded as a segment with `detail["truncated"]` and a text in `metadata.problems`. Only input
//! that is not the format at all returns `Err`. Every top-level segment list covers the file from byte 0 to its
//! end without gaps (unparsed bytes become "garbage"/"trailer" segments), so every byte is accounted for.

pub mod heif;
pub(crate) mod inflate;
pub mod jpeg;
pub mod meta;
pub mod png;
pub mod signals;
pub mod webp;

use crate::model::{Format, ParsedImage, Segment};
use std::collections::BTreeMap;

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    match Format::sniff(data) {
        Format::Jpeg => jpeg::parse(data),
        Format::Png => png::parse(data),
        Format::Webp => webp::parse(data),
        Format::Heif => heif::parse(data),
        Format::Unknown => Err("not a JPEG, PNG, WebP or HEIF file".into()),
    }
}

// ---- small checked readers (never panic: out of range → None) ----

pub(crate) fn be16(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}
pub(crate) fn be32(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
pub(crate) fn be64(d: &[u8], o: usize) -> Option<u64> {
    let b = d.get(o..o.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    Some(u64::from_be_bytes(a))
}
pub(crate) fn le16(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}
pub(crate) fn le24(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(3)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], 0]))
}
pub(crate) fn le32(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A FourCC / chunk type as text (non-printable bytes as `\xNN`).
pub(crate) fn fourcc(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        if (0x20..0x7f).contains(&c) {
            s.push(c as char);
        } else {
            s.push_str(&format!("\\x{c:02X}"));
        }
    }
    s
}

/// Latin-1 / lossy text, trimmed of NULs and spaces, capped at `max` characters.
pub(crate) fn text(b: &[u8], max: usize) -> String {
    let s: String = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    };
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    cap(s, max)
}

pub(crate) fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

pub(crate) fn seg(kind: &str, offset: usize, length: usize) -> Segment {
    Segment {
        kind: kind.to_string(),
        offset: offset as u64,
        length: length as u64,
        detail: BTreeMap::new(),
        children: vec![],
    }
}

pub(crate) trait SegExt {
    fn d(self, k: &str, v: impl ToString) -> Self;
}
impl SegExt for Segment {
    fn d(mut self, k: &str, v: impl ToString) -> Segment {
        self.detail.insert(k.to_string(), v.to_string());
        self
    }
}

/// An empty image of `format` for the parsers to fill.
pub(crate) fn empty(format: Format) -> ParsedImage {
    ParsedImage {
        format,
        width: 0,
        height: 0,
        structure: vec![],
        metadata: Default::default(),
        jpeg: None,
        png: None,
        webp: None,
        heif: None,
        thumbnail: None,
        signals: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_never_panic_out_of_range() {
        let d = [1u8, 2, 3];
        assert_eq!(be16(&d, 1), Some(0x0203));
        assert_eq!(be16(&d, 2), None);
        assert_eq!(be32(&d, 0), None);
        assert_eq!(be64(&d, usize::MAX), None);
        assert_eq!(le32(&d, usize::MAX - 1), None);
        assert_eq!(le24(&d, 0), Some(0x030201));
    }

    #[test]
    fn text_helpers() {
        assert_eq!(fourcc(b"IH\x00R"), "IH\\x00R");
        assert_eq!(text(b"GIMP 2.10\0\0", 50), "GIMP 2.10");
        assert_eq!(text(&[0xE9, b'a'], 50), "éa");
        assert_eq!(cap("abcdef", 3), "abc…");
    }

    #[test]
    fn unknown_format_is_an_error() {
        assert!(parse(b"hello").is_err());
        assert!(parse(&[]).is_err());
    }
}
