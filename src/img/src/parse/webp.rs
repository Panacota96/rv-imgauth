//! WebP RIFF walker (RFC 9649): VP8 (lossy), VP8L (lossless) and VP8X (extended) headers with dimensions and
//! flags, EXIF/XMP chunks (→ readers), ICCP (hash), unknown chunks, and the consistency between the VP8X flags,
//! the canvas size and the chunks present.

use super::{SegExt, empty, fourcc, le16, le24, le32, meta, seg, signals::sig};
use crate::model::{Format, Metadata, ParsedImage, Signal, WebpInfo};

const KNOWN: &[&str] = &[
    "VP8 ", "VP8L", "VP8X", "ALPH", "ANIM", "ANMF", "ICCP", "EXIF", "XMP ",
];

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err("not a WebP file".into());
    }
    let mut p = empty(Format::Webp);
    let mut info = WebpInfo::default();
    let mut md = Metadata::default();
    let mut thumb = None;
    let len = data.len();
    let riff_size = le32(data, 4).unwrap_or(0) as usize;
    let riff_end = 8usize.saturating_add(riff_size);
    let end = riff_end.min(len);
    let mut segs = vec![seg("RIFF header", 0, 12).d("riff_size", riff_size)];
    if riff_end > len {
        md.problems.push(format!(
            "WebP structure: the RIFF header declares {riff_size} bytes but the file has {} (file truncated)",
            len - 8
        ));
    }
    let mut problems: Vec<String> = vec![];
    let mut canvas: Option<(u32, u32)> = None;
    let mut flags_byte: Option<u8> = None;
    let mut bitstream: Option<(String, u32, u32)> = None;
    let mut vp8l_alpha = false;
    let mut pos = 12usize;
    while pos < end {
        let (Some(ty), Some(size)) = (data.get(pos..pos + 4), le32(data, pos + 4)) else {
            segs.push(seg("garbage", pos, end - pos).d("truncated", "true"));
            md.problems.push(format!(
                "WebP structure: {} byte(s) at offset {pos} are too short for a chunk header",
                end - pos
            ));
            break;
        };
        let kind = fourcc(ty);
        let size = size as usize;
        let padded = size + (size & 1);
        let body_end = pos + 8 + size;
        if body_end > end {
            segs.push(
                seg(kind.trim_end(), pos, end - pos)
                    .d("truncated", "true")
                    .d("declared_length", size),
            );
            info.chunks.push(kind.trim_end().to_string());
            md.problems.push(format!(
                "WebP structure: chunk {} at offset {pos} declares {size} bytes but only {} remain",
                kind.trim_end(),
                end.saturating_sub(pos + 8)
            ));
            break;
        }
        let body = &data[pos + 8..body_end];
        let chunk_len = (8 + padded).min(end - pos);
        let mut s = seg(kind.trim_end(), pos, chunk_len);
        match kind.as_str() {
            "VP8X" if size >= 10 => {
                let f = body[0];
                flags_byte = Some(f);
                let w = le24(body, 4).unwrap_or(0) + 1;
                let h = le24(body, 7).unwrap_or(0) + 1;
                canvas = Some((w, h));
                for (bit, name) in [
                    (0x20, "ICC"),
                    (0x10, "ALPHA"),
                    (0x08, "EXIF"),
                    (0x04, "XMP"),
                    (0x02, "ANIMATION"),
                ] {
                    if f & bit != 0 {
                        info.flags.push(name.into());
                    }
                }
                s = s
                    .d("flags", format!("0x{f:02X}"))
                    .d("canvas", format!("{w}x{h}"));
                if pos != 12 {
                    problems.push("the VP8X chunk is not the first chunk".into());
                }
            }
            "VP8 " if size >= 10 => {
                let tag = le24(body, 0).unwrap_or(0);
                let key = tag & 1 == 0;
                if key && body[3..6] == [0x9D, 0x01, 0x2A] {
                    let w = (le16(body, 6).unwrap_or(0) & 0x3FFF) as u32;
                    let h = (le16(body, 8).unwrap_or(0) & 0x3FFF) as u32;
                    s = s.d("width", w).d("height", h);
                    if bitstream.is_none() {
                        bitstream = Some(("VP8".into(), w, h));
                    }
                } else {
                    md.problems.push(format!(
                        "WebP: VP8 chunk at offset {pos} has no key-frame header"
                    ));
                }
            }
            "VP8L" if size >= 5 => {
                if body[0] == 0x2F {
                    let b = le32(body, 1).unwrap_or(0);
                    let w = (b & 0x3FFF) + 1;
                    let h = ((b >> 14) & 0x3FFF) + 1;
                    vp8l_alpha = (b >> 28) & 1 == 1;
                    s = s.d("width", w).d("height", h).d("alpha_hint", vp8l_alpha);
                    if bitstream.is_none() {
                        bitstream = Some(("VP8L".into(), w, h));
                    }
                } else {
                    md.problems.push(format!(
                        "WebP: VP8L chunk at offset {pos} has a bad signature"
                    ));
                }
            }
            "EXIF" => {
                if info.chunks.iter().any(|c| c == "EXIF") {
                    problems.push("more than one EXIF chunk".into());
                }
                let t = meta::read_exif(body, &mut md, "WebP EXIF");
                if thumb.is_none() {
                    thumb = t;
                }
            }
            "XMP " => meta::read_xmp(&String::from_utf8_lossy(body), &mut md),
            "ICCP" => md.icc_sha256 = Some(meta::sha256_hex(body)),
            _ => {}
        }
        if !KNOWN.contains(&kind.as_str()) {
            s = s.d("unknown", "true");
        }
        info.chunks.push(kind.trim_end().to_string());
        segs.push(s);
        pos += chunk_len;
    }
    if riff_end < len {
        segs.push(seg("trailer", riff_end, len - riff_end));
    }
    // kind, size
    let has = |c: &str| info.chunks.iter().any(|x| x == c);
    info.kind = if canvas.is_some() {
        "VP8X".into()
    } else {
        bitstream.as_ref().map(|b| b.0.clone()).unwrap_or_default()
    };
    info.lossless = bitstream.as_ref().is_some_and(|b| b.0 == "VP8L");
    let (w, h) = canvas
        .or(bitstream.as_ref().map(|b| (b.1, b.2)))
        .unwrap_or((0, 0));
    info.width = w;
    info.height = h;
    // flag/chunk consistency
    if let Some(f) = flags_byte {
        let anim = f & 0x02 != 0;
        for (bit, flag, chunk) in [
            (0x20u8, "ICC", "ICCP"),
            (0x08, "EXIF", "EXIF"),
            (0x04, "XMP", "XMP"),
            (0x02, "Animation", "ANIM"),
        ] {
            let set = f & bit != 0;
            if set && !has(chunk) {
                problems.push(format!(
                    "the {flag} flag is set but there is no {chunk} chunk"
                ));
            } else if !set && has(chunk) {
                problems.push(format!(
                    "there is a {chunk} chunk but the {flag} flag is not set"
                ));
            }
        }
        if f & 0x10 == 0 && has("ALPH") {
            problems.push("there is an ALPH chunk but the Alpha flag is not set".into());
        }
        if f & 0x10 != 0 && !anim && !has("ALPH") && !(info.lossless && vp8l_alpha) {
            problems.push("the Alpha flag is set but the image carries no alpha data".into());
        }
        if !anim
            && let (Some((cw, ch)), Some((_, bw, bh))) = (canvas, bitstream.as_ref())
            && (cw != *bw || ch != *bh)
        {
            problems.push(format!(
                "the canvas is {cw}×{ch} but the image bitstream is {bw}×{bh}"
            ));
        }
        if has("ICCP") && info.chunks.get(1).map(String::as_str) != Some("ICCP") {
            problems.push("the ICCP chunk does not follow VP8X".into());
        }
    } else {
        for c in ["EXIF", "XMP", "ICCP", "ALPH", "ANIM"] {
            if has(c) {
                problems.push(format!(
                    "a {c} chunk is present in a simple-format file (no VP8X)"
                ));
            }
        }
    }
    if bitstream.is_none() && !has("ANMF") {
        md.problems
            .push("WebP structure: no VP8 or VP8L image data found".into());
    }
    let mut sigs: Vec<Signal> = vec![];
    if !problems.is_empty() {
        sigs.push(sig("WEBP_FLAGS_MISMATCH").with("problems", problems.join("; ")));
    }
    if riff_end < len {
        sigs.push(
            sig("DATA_AFTER_END_MARKER")
                .with("bytes", (len - riff_end).to_string())
                .with("format", "WebP"),
        );
    }
    p.width = w;
    p.height = h;
    p.structure = segs;
    p.metadata = md;
    p.thumbnail = thumb;
    p.webp = Some(info);
    p.signals = sigs;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(ty: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = ty.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }
    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend(((body.len() + 4) as u32).to_le_bytes());
        v.extend(b"WEBP");
        v.extend(body);
        v
    }
    fn vp8(w: u16, h: u16) -> Vec<u8> {
        let mut b = vec![0x10, 0x02, 0x00, 0x9D, 0x01, 0x2A];
        b.extend(w.to_le_bytes());
        b.extend(h.to_le_bytes());
        b.extend([0u8; 7]);
        chunk(b"VP8 ", &b)
    }
    fn vp8x(flags: u8, w: u32, h: u32) -> Vec<u8> {
        let mut b = vec![flags, 0, 0, 0];
        b.extend(&(w - 1).to_le_bytes()[..3]);
        b.extend(&(h - 1).to_le_bytes()[..3]);
        chunk(b"VP8X", &b)
    }

    #[test]
    fn simple_lossy() {
        let f = riff(&[vp8(64, 48)]);
        let p = parse(&f).unwrap();
        assert_eq!((p.width, p.height), (64, 48));
        let w = p.webp.unwrap();
        assert_eq!(w.kind, "VP8");
        assert!(!w.lossless);
        assert!(p.signals.is_empty());
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total as usize, f.len());
    }

    #[test]
    fn flags_mismatch_and_size() {
        let f = riff(&[
            vp8x(0x08, 64, 50),
            vp8(64, 48),
            chunk(b"XMP ", b"<x/>"),
            chunk(b"ZZZZ", b"abc"),
        ]);
        let p = parse(&f).unwrap();
        assert_eq!(p.webp.as_ref().unwrap().flags, ["EXIF"]);
        let s = &p.signals[0];
        assert_eq!(s.code, "WEBP_FLAGS_MISMATCH");
        let pr = &s.params["problems"];
        assert!(pr.contains("EXIF flag is set"), "{pr}");
        assert!(pr.contains("XMP chunk but"), "{pr}");
        assert!(pr.contains("64×50"), "{pr}");
        assert_eq!(p.structure.last().unwrap().detail["unknown"], "true");
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total as usize, f.len());
    }

    #[test]
    fn vp8l_and_trailer_and_truncation() {
        let mut b = vec![0x2F];
        b.extend(((9u32) | (19 << 14) | (1 << 28)).to_le_bytes());
        let mut f = riff(&[chunk(b"VP8L", &b)]);
        f.extend(b"extra");
        let p = parse(&f).unwrap();
        assert_eq!((p.width, p.height), (10, 20));
        assert!(p.webp.as_ref().unwrap().lossless);
        assert_eq!(p.signals[0].code, "DATA_AFTER_END_MARKER");
        assert_eq!(p.signals[0].params["bytes"], "5");
        // truncated chunk
        let f = riff(&[vp8(64, 48)]);
        let p = parse(&f[..20]).unwrap();
        assert!(!p.metadata.problems.is_empty());
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total, 20);
        // huge size
        let mut f = riff(&[]);
        f.extend(b"VP8 \xff\xff\xff\xff");
        let p = parse(&f).unwrap();
        assert!(!p.metadata.problems.is_empty());
    }
}
