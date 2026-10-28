//! PNG chunk walker (PNG 3rd edition, W3C REC 2025-06-24): CRC check, IHDR, text chunks (tEXt, zTXt, iTXt with
//! compressed text inflated by the small decoder in `inflate`), tIME, eXIf (→ EXIF reader), XMP in iTXt
//! "XML:com.adobe.xmp", iCCP (SHA-256 of the decompressed profile), ImageMagick "Raw profile type exif" text,
//! and bytes after IEND. IDAT is never decompressed here.

use super::{SegExt, be16, be32, empty, fourcc, inflate, meta, seg, signals, text};
use crate::model::{Format, Metadata, ParsedImage, PngInfo, Tag};

const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Cap for decompressed text/ICC chunks.
const TEXT_LIMIT: usize = 16 << 20;

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *e = c;
    }
    t
}

pub(crate) fn crc32(t: &[u32; 256], parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for p in parts {
        for &b in *p {
            c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn split_nul(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = b.iter().position(|&c| c == 0)?;
    Some((&b[..i], &b[i + 1..]))
}

fn push_text(md: &mut Metadata, group: &str, key: &str, value: &str) {
    md.tags.push(Tag {
        group: group.into(),
        id: 0,
        name: key.to_string(),
        value: super::cap(value.trim_matches('\0'), 2000),
    });
}

/// "Raw profile type exif/APP1" (ImageMagick): "\nexif\n   length\nhex…".
fn raw_profile(s: &str) -> Option<Vec<u8>> {
    let mut lines = s.trim_start_matches('\n').splitn(3, '\n');
    let _name = lines.next()?;
    let len: usize = lines.next()?.trim().parse().ok()?;
    let hex: Vec<u8> = lines
        .next()?
        .bytes()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let bytes: Vec<u8> = hex
        .chunks_exact(2)
        .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect();
    let _ = len;
    (!bytes.is_empty()).then_some(bytes)
}

/// Text chunk value handling shared by tEXt/zTXt/iTXt.
fn text_value(md: &mut Metadata, group: &str, key: &str, value: &str, thumb: &mut Option<Vec<u8>>) {
    if key == "XML:com.adobe.xmp" {
        meta::read_xmp(value, md);
        return;
    }
    if let Some(kind) = key.strip_prefix("Raw profile type ") {
        if kind.eq_ignore_ascii_case("exif") || kind.eq_ignore_ascii_case("APP1") {
            match raw_profile(value) {
                Some(b) => {
                    let t = meta::read_exif(&b, md, "PNG raw profile");
                    if thumb.is_none() {
                        *thumb = t;
                    }
                }
                None => md
                    .problems
                    .push(format!("PNG: the \"{key}\" text could not be decoded")),
            }
            push_text(md, group, key, &format!("<{} characters>", value.len()));
            return;
        }
        if kind.eq_ignore_ascii_case("iptc") || kind.eq_ignore_ascii_case("8bim") {
            if let Some(b) = raw_profile(value) {
                if kind.eq_ignore_ascii_case("iptc") {
                    meta::read_iptc(&b, md);
                } else {
                    meta::read_photoshop_irb(&b, 0, md);
                }
            }
            push_text(md, group, key, &format!("<{} characters>", value.len()));
            return;
        }
    }
    push_text(md, group, key, value);
}

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    if !data.starts_with(SIG) {
        return Err("not a PNG file".into());
    }
    let crc_t = crc_table();
    let mut p = empty(Format::Png);
    let mut info = PngInfo::default();
    let mut md = Metadata::default();
    let mut thumb = None;
    let mut segs = vec![seg("signature", 0, 8)];
    let len = data.len();
    let mut pos = 8usize;
    let mut iend = false;
    while pos < len {
        let (Some(clen), Some(ty)) = (be32(data, pos), data.get(pos + 4..pos + 8)) else {
            segs.push(seg("garbage", pos, len - pos).d("truncated", "true"));
            md.problems.push(format!(
                "PNG structure: {} byte(s) at offset {pos} are too short for a chunk header (file truncated)",
                len - pos
            ));
            break;
        };
        let kind = fourcc(ty);
        let clen = clen as usize;
        if clen > 0x7FFF_FFFF || pos + 12 + clen > len {
            segs.push(
                seg(&kind, pos, len - pos)
                    .d("truncated", "true")
                    .d("declared_length", clen),
            );
            info.chunks.push(kind.clone());
            md.problems.push(format!(
                "PNG structure: chunk {kind} at offset {pos} declares {clen} bytes but only {} remain (file truncated)",
                len.saturating_sub(pos + 12)
            ));
            break;
        }
        let body = &data[pos + 8..pos + 8 + clen];
        let crc = be32(data, pos + 8 + clen).unwrap_or(0);
        let mut s = seg(&kind, pos, 12 + clen);
        if crc32(&crc_t, &[ty, body]) != crc {
            s = s.d("crc", "mismatch");
            md.problems.push(format!(
                "PNG structure: chunk {kind} at offset {pos} fails its CRC check"
            ));
        }
        info.chunks.push(kind.clone());
        match kind.as_str() {
            "IHDR" if clen >= 13 => {
                info.width = be32(body, 0).unwrap_or(0);
                info.height = be32(body, 4).unwrap_or(0);
                info.bit_depth = body[8];
                info.color_type = body[9];
                s = s
                    .d("width", info.width)
                    .d("height", info.height)
                    .d("bit_depth", body[8])
                    .d("color_type", body[9])
                    .d("interlace", body[12]);
            }
            "tEXt" => match split_nul(body) {
                Some((k, v)) => {
                    let k = latin1(k);
                    s = s.d("keyword", &k);
                    text_value(&mut md, "PNG-tEXt", &k, &latin1(v), &mut thumb);
                }
                None => md.problems.push(format!(
                    "PNG: tEXt at offset {pos} has no keyword separator"
                )),
            },
            "zTXt" => match split_nul(body) {
                Some((k, rest)) if !rest.is_empty() => {
                    let k = latin1(k);
                    s = s.d("keyword", &k);
                    match inflate::zlib(&rest[1..], TEXT_LIMIT) {
                        Ok(t) => text_value(&mut md, "PNG-zTXt", &k, &latin1(&t), &mut thumb),
                        Err(e) => md
                            .problems
                            .push(format!("PNG: zTXt \"{k}\" could not be decompressed ({e})")),
                    }
                }
                _ => md
                    .problems
                    .push(format!("PNG: zTXt at offset {pos} is malformed")),
            },
            "iTXt" => {
                // keyword\0 flag method lang\0 translated\0 text
                let parsed = split_nul(body).and_then(|(k, r)| {
                    let (&flag, r) = r.split_first()?;
                    let (_method, r) = r.split_first()?;
                    let (lang, r) = split_nul(r)?;
                    let (_tr, t) = split_nul(r)?;
                    Some((latin1(k), flag, latin1(lang), t))
                });
                match parsed {
                    Some((k, flag, lang, t)) => {
                        s = s.d("keyword", &k);
                        if !lang.is_empty() {
                            s = s.d("language", lang);
                        }
                        let t = if flag == 1 {
                            match inflate::zlib(t, TEXT_LIMIT) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    md.problems.push(format!(
                                        "PNG: iTXt \"{k}\" could not be decompressed ({e})"
                                    ));
                                    None
                                }
                            }
                        } else {
                            Some(t.to_vec())
                        };
                        if let Some(t) = t {
                            let v = String::from_utf8_lossy(&t).into_owned();
                            text_value(&mut md, "PNG-iTXt", &k, &v, &mut thumb);
                        }
                    }
                    None => md
                        .problems
                        .push(format!("PNG: iTXt at offset {pos} is malformed")),
                }
            }
            "tIME" if clen >= 7 => {
                let t = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    be16(body, 0).unwrap_or(0),
                    body[2],
                    body[3],
                    body[4],
                    body[5],
                    body[6]
                );
                s = s.d("time", &t);
                push_text(&mut md, "PNG-tIME", "tIME", &t);
            }
            "eXIf" => {
                if body.starts_with(b"Exif\0\0") {
                    md.problems
                        .push("PNG: the eXIf chunk starts with \"Exif\\0\\0\" (not allowed by the specification)".into());
                }
                let t = meta::read_exif(body, &mut md, "PNG eXIf");
                if thumb.is_none() {
                    thumb = t;
                }
            }
            "iCCP" => match split_nul(body) {
                Some((name, rest)) if !rest.is_empty() => {
                    s = s.d("profile_name", text(name, 80));
                    match inflate::zlib(&rest[1..], TEXT_LIMIT) {
                        Ok(icc) => md.icc_sha256 = Some(meta::sha256_hex(&icc)),
                        Err(e) => md.problems.push(format!(
                            "PNG: the iCCP profile could not be decompressed ({e})"
                        )),
                    }
                }
                _ => md.problems.push("PNG: iCCP is malformed".into()),
            },
            "pHYs" if clen >= 9 => {
                s = s.d(
                    "density",
                    format!(
                        "{}x{} unit {}",
                        be32(body, 0).unwrap_or(0),
                        be32(body, 4).unwrap_or(0),
                        body[8]
                    ),
                );
            }
            _ => {}
        }
        segs.push(s);
        pos += 12 + clen;
        if kind == "IEND" {
            iend = true;
            break;
        }
    }
    if iend && pos < len {
        info.after_iend = (len - pos) as u64;
        segs.push(seg("trailer", pos, len - pos));
    } else if !iend && !md.problems.iter().any(|p| p.contains("truncated")) {
        md.problems
            .push("PNG structure: the file ends without an IEND chunk".into());
    }
    let order = signals::png_order_problems(&info.chunks);
    p.width = info.width;
    p.height = info.height;
    p.structure = segs;
    p.metadata = md;
    p.thumbnail = thumb;
    p.png = Some(info);
    p.signals = signals::png_structure(&p, &order);
    Ok(p)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn chunk(ty: &[u8], body: &[u8]) -> Vec<u8> {
        let t = crc_table();
        let mut v = (body.len() as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(body);
        v.extend(crc32(&t, &[ty, body]).to_be_bytes());
        v
    }

    fn ihdr(w: u32, h: u32) -> Vec<u8> {
        let mut b = w.to_be_bytes().to_vec();
        b.extend(h.to_be_bytes());
        b.extend([8, 2, 0, 0, 0]);
        chunk(b"IHDR", &b)
    }

    #[test]
    fn crc_known_value() {
        // CRC of "IEND" with empty body is AE426082
        assert_eq!(crc32(&crc_table(), &[b"IEND"]), 0xAE42_6082);
    }

    #[test]
    fn minimal_png_and_text() {
        let mut f = SIG.to_vec();
        f.extend(ihdr(3, 2));
        f.extend(chunk(b"tEXt", b"Software\0Screenshot tool"));
        f.extend(chunk(b"tIME", &[0x07, 0xE8, 3, 10, 12, 0, 1]));
        // zTXt with zlib "hello hello hello hello"
        let mut z = b"Comment\0\0".to_vec();
        z.extend([
            120, 156, 203, 72, 205, 201, 201, 87, 200, 64, 39, 1, 104, 3, 8, 177,
        ]);
        f.extend(chunk(b"zTXt", &z));
        // iTXt uncompressed
        f.extend(chunk(b"iTXt", b"Title\0\0\0en\0Titel\0caf\xC3\xA9"));
        f.extend(chunk(b"IDAT", &[0; 4]));
        f.extend(chunk(b"IEND", &[]));
        let p = parse(&f).unwrap();
        assert_eq!((p.width, p.height), (3, 2));
        assert_eq!(
            p.metadata.get("PNG-tEXt", "Software"),
            Some("Screenshot tool")
        );
        assert_eq!(
            p.metadata.get("PNG-tIME", "tIME"),
            Some("2024-03-10 12:00:01")
        );
        assert_eq!(
            p.metadata.get("PNG-zTXt", "Comment"),
            Some("hello hello hello hello")
        );
        assert_eq!(p.metadata.get("PNG-iTXt", "Title"), Some("café"));
        assert!(p.metadata.problems.is_empty(), "{:?}", p.metadata.problems);
        assert!(p.signals.is_empty(), "{:?}", p.signals);
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total as usize, f.len());
    }

    #[test]
    fn bad_crc_trailer_private_and_order() {
        let mut f = SIG.to_vec();
        f.extend(ihdr(1, 1));
        f.extend(chunk(b"IDAT", &[0; 4]));
        let mut bad = chunk(b"tEXt", b"a\0b");
        let n = bad.len();
        bad[n - 1] ^= 1;
        f.extend(bad);
        f.extend(chunk(b"IDAT", &[0; 4]));
        f.extend(chunk(b"prVt", b"x"));
        f.extend(chunk(b"IEND", &[]));
        f.extend(b"TRAILING");
        let p = parse(&f).unwrap();
        assert!(p.metadata.problems.iter().any(|x| x.contains("CRC")));
        let codes: Vec<&str> = p.signals.iter().map(|s| s.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "PNG_CHUNK_ORDER_INVALID",
                "PNG_PRIVATE_CHUNKS",
                "DATA_AFTER_END_MARKER"
            ]
        );
        assert_eq!(p.png.as_ref().unwrap().after_iend, 8);
        assert_eq!(p.structure.last().unwrap().kind, "trailer");
    }

    #[test]
    fn truncated_and_huge_chunks() {
        let mut f = SIG.to_vec();
        f.extend(ihdr(1, 1));
        f.extend(0xFFFF_FFF0u32.to_be_bytes());
        f.extend(b"IDAT");
        f.extend([1, 2, 3]);
        let p = parse(&f).unwrap();
        assert_eq!(p.structure.last().unwrap().detail["truncated"], "true");
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total as usize, f.len());
        // header cut
        let p = parse(&f[..f.len() - 9]).unwrap();
        assert!(!p.metadata.problems.is_empty());
        assert!(parse(&SIG[..7]).is_err());
    }

    #[test]
    fn raw_profile_text() {
        let hex = "457869660000"; // only "Exif\0\0": EXIF reader reports a problem, no panic
        let v = format!("\nexif\n       6\n{hex}\n");
        assert_eq!(raw_profile(&v).unwrap(), b"Exif\0\0");
    }
}
