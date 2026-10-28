//! JPEG (ITU-T T.81) marker parser and baseline Huffman entropy decoder.
//!
//! `parse` walks SOI … EOI: every marker segment becomes a [`Segment`] (APPn named by identifier, e.g.
//! "APP1/Exif"), the entropy-coded data after each SOS becomes an "ECS" segment (byte stuffing and RSTn skipped,
//! RST count in its detail), bytes between segments become "garbage", and bytes after EOI become "MPF image"
//! segments (secondary images listed by a valid APP2/MPF index) or a "trailer". The top-level segments
//! therefore cover the file without gaps.
//!
//! `decode_coefficients` decodes sequential Huffman scans (SOF0/SOF1), interleaved or not, with restart
//! intervals, into quantized DCT coefficients in natural order. The coefficient grid of each component covers
//! whole MCUs (`blocks_wide = MCUs across × H`), so padding blocks right and below the image are included;
//! blocks a non-interleaved scan does not code stay zero.

use super::{SegExt, be16, be32, cap, empty, meta, seg, text};
use crate::model::{
    Component, ComponentCoefficients, Format, HuffmanTable, JpegInfo, Metadata, ParsedImage,
    QuantTable, Segment,
};

/// Zig-zag position k → natural (row-major) index.
pub const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// One scan as found by the walker.
#[derive(Clone, Debug)]
pub(crate) struct Scan {
    /// (component id, DC table, AC table)
    pub comps: Vec<(u8, u8, u8)>,
    pub ss: u8,
    pub se: u8,
    pub ah: u8,
    pub al: u8,
    pub ecs: (usize, usize),
}

/// Table and scan events in file order (the decoder replays them: tables may change between scans).
#[derive(Clone, Debug)]
pub(crate) enum Ev {
    Dht(HuffmanTable),
    Dri(u16),
    Sos(Scan),
}

/// MPF index entry (CIPA DC-007): absolute offset and size of each image.
#[derive(Clone, Debug)]
struct MpEntry {
    offset: usize,
    size: usize,
}

pub(crate) struct Walk {
    pub info: JpegInfo,
    pub segs: Vec<Segment>,
    pub md: Metadata,
    pub thumbnail: Option<Vec<u8>>,
    pub events: Vec<Ev>,
    /// End of EOI (None when no EOI was found).
    pub eoi_end: Option<usize>,
    /// Bytes after EOI that are MPF secondary images (validated SOI at the listed offsets).
    pub mpf_bytes: u64,
    /// Truncation/damage noticed while walking (also copied to md.problems).
    pub damaged: bool,
}

fn app_kind(n: u8, p: &[u8]) -> String {
    let ident_len = p.iter().take(40).position(|&b| b == 0);
    let ident = ident_len
        .map(|l| &p[..l])
        .filter(|s| s.iter().all(|&b| (0x20..0x7f).contains(&b)))
        .map(|s| String::from_utf8_lossy(s).into_owned());
    if n == 1 && p.starts_with(b"Exif\0") {
        return "APP1/Exif".into();
    }
    match (n, ident.as_deref()) {
        (0, Some("JFIF")) => "APP0/JFIF".into(),
        (0, Some("JFXX")) => "APP0/JFXX".into(),
        (1, Some("http://ns.adobe.com/xap/1.0/")) => "APP1/XMP".into(),
        (1, Some("http://ns.adobe.com/xmp/extension/")) => "APP1/XMP-extension".into(),
        (2, Some("ICC_PROFILE")) => "APP2/ICC_PROFILE".into(),
        (2, Some("MPF")) => "APP2/MPF".into(),
        (13, Some("Photoshop 3.0")) => "APP13/Photoshop".into(),
        (12, Some("Ducky")) => "APP12/Ducky".into(),
        (14, _) if p.starts_with(b"Adobe") => "APP14/Adobe".into(),
        (_, Some(id)) if !id.is_empty() => format!("APP{n}/{}", cap(id, 24)),
        _ => format!("APP{n}"),
    }
}

fn sof_name(m: u8) -> Option<String> {
    match m {
        0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => Some(format!("SOF{}", m - 0xC0)),
        _ => None,
    }
}

fn subsampling(comps: &[Component]) -> String {
    match comps.len() {
        1 => "grey".into(),
        3 => {
            let (y, cb, cr) = (&comps[0], &comps[1], &comps[2]);
            if cb.h != cr.h || cb.v != cr.v || cb.h == 0 || cb.v == 0 {
                return "other".into();
            }
            if y.h % cb.h != 0 || y.v % cb.v != 0 {
                return "other".into();
            }
            match (y.h / cb.h, y.v / cb.v) {
                (1, 1) => "4:4:4",
                (2, 1) => "4:2:2",
                (2, 2) => "4:2:0",
                (1, 2) => "4:4:0",
                (4, 1) => "4:1:1",
                _ => "other",
            }
            .into()
        }
        _ => "other".into(),
    }
}

/// Walks the marker structure. `Err` only when the data does not start with SOI.
pub(crate) fn walk(data: &[u8]) -> Result<Walk, String> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG file (no SOI marker)".into());
    }
    let mut w = Walk {
        info: JpegInfo::default(),
        segs: vec![seg("SOI", 0, 2)],
        md: Metadata::default(),
        thumbnail: None,
        events: vec![],
        eoi_end: None,
        mpf_bytes: 0,
        damaged: false,
    };
    w.info.markers.push("SOI".into());
    let len = data.len();
    let mut icc: Vec<(u8, u8, Vec<u8>)> = vec![];
    let mut mpf: Vec<MpEntry> = vec![];
    let mut exif_seen = false;
    let mut pos = 2usize;
    loop {
        if pos >= len {
            w.md.problems.push(format!(
                "JPEG structure: the file ends at byte {len} without an EOI marker"
            ));
            w.damaged = true;
            break;
        }
        if data[pos] != 0xFF {
            let next = data[pos..]
                .iter()
                .position(|&b| b == 0xFF)
                .map(|i| pos + i)
                .unwrap_or(len);
            w.segs.push(seg("garbage", pos, next - pos));
            w.md.problems.push(format!(
                "JPEG structure: {} unexpected byte(s) at offset {pos} between segments",
                next - pos
            ));
            w.damaged = true;
            pos = next;
            continue;
        }
        let mut p = pos;
        while p + 1 < len && data[p + 1] == 0xFF {
            p += 1;
        }
        if p + 1 >= len {
            w.segs.push(seg("garbage", pos, len - pos));
            w.md.problems.push(format!(
                "JPEG structure: the file ends inside a marker at offset {pos}"
            ));
            w.damaged = true;
            pos = len;
            continue;
        }
        let m = data[p + 1];
        match m {
            0xD8 | 0xD9 | 0xD0..=0xD7 | 0x01 | 0x00 => {
                let name = match m {
                    0xD8 => "SOI".to_string(),
                    0xD9 => "EOI".to_string(),
                    0x01 => "TEM".to_string(),
                    0x00 => "garbage".to_string(),
                    _ => format!("RST{}", m - 0xD0),
                };
                w.segs.push(seg(&name, pos, p + 2 - pos));
                if m == 0x00 {
                    w.md.problems
                        .push(format!("JPEG structure: stray 0xFF00 at offset {pos}"));
                    w.damaged = true;
                } else {
                    w.info.markers.push(name);
                }
                pos = p + 2;
                if m == 0xD9 {
                    w.eoi_end = Some(pos);
                    break;
                }
                continue;
            }
            _ => {}
        }
        let Some(l) = be16(data, p + 2) else {
            w.segs
                .push(seg(&format!("0xFF{m:02X}"), pos, len - pos).d("truncated", "true"));
            w.md.problems.push(format!(
                "JPEG structure: the file ends inside the length of the segment at offset {pos}"
            ));
            w.damaged = true;
            break;
        };
        let l = l as usize;
        let kind = match m {
            0xE0..=0xEF => app_kind(
                m - 0xE0,
                data.get(p + 4..(p + 2 + l).min(len)).unwrap_or(&[]),
            ),
            0xDB => "DQT".into(),
            0xC4 => "DHT".into(),
            0xDD => "DRI".into(),
            0xDA => "SOS".into(),
            0xFE => "COM".into(),
            0xCC => "DAC".into(),
            0xDC => "DNL".into(),
            0xDE => "DHP".into(),
            0xDF => "EXP".into(),
            _ => sof_name(m).unwrap_or_else(|| format!("0xFF{m:02X}")),
        };
        if l < 2 {
            w.segs
                .push(seg(&kind, pos, p + 4 - pos).d("malformed", "length < 2"));
            w.segs
                .push(seg("garbage", p + 4, len.saturating_sub(p + 4)));
            if p + 4 > len {
                // cannot happen (length was read), kept for clarity
            }
            w.md.problems.push(format!(
                "JPEG structure: segment {kind} at offset {pos} has an invalid length {l}"
            ));
            w.damaged = true;
            break;
        }
        let end = p + 2 + l;
        if end > len {
            w.segs.push(
                seg(&kind, pos, len - pos)
                    .d("truncated", "true")
                    .d("declared_length", l),
            );
            w.info.markers.push(kind.clone());
            w.md.problems.push(format!(
                "JPEG structure: segment {kind} at offset {pos} declares {l} bytes but only {} remain (file truncated)",
                len - p - 2
            ));
            w.damaged = true;
            break;
        }
        let payload = &data[p + 4..end];
        let mut s = seg(&kind, pos, end - pos);
        w.info.markers.push(kind.clone());
        match m {
            0xE0..=0xEF => match kind.as_str() {
                "APP0/JFIF" if payload.len() >= 12 => {
                    s = s
                        .d("version", format!("{}.{:02}", payload[5], payload[6]))
                        .d(
                            "density",
                            format!(
                                "{}x{} unit {}",
                                be16(payload, 8).unwrap_or(0),
                                be16(payload, 10).unwrap_or(0),
                                payload[7]
                            ),
                        );
                }
                "APP1/Exif" => {
                    if exif_seen {
                        w.md.problems.push(format!(
                            "a second EXIF APP1 segment at offset {pos} was also read"
                        ));
                    }
                    exif_seen = true;
                    let tiff = payload.get(6..).unwrap_or(&[]);
                    let t = meta::read_exif(tiff, &mut w.md, "APP1");
                    if w.thumbnail.is_none() {
                        w.thumbnail = t;
                    }
                }
                "APP1/XMP" => {
                    let pk = payload.get(29..).unwrap_or(&[]);
                    meta::read_xmp(&String::from_utf8_lossy(pk), &mut w.md);
                }
                "APP2/ICC_PROFILE" if payload.len() >= 14 => {
                    icc.push((payload[12], payload[13], payload[14..].to_vec()));
                    s = s.d("chunk", format!("{}/{}", payload[12], payload[13]));
                }
                "APP2/MPF" => {
                    mpf = read_mpf(payload, p + 4 + 4, &mut w.md);
                    s = s.d("images", mpf.len());
                }
                "APP13/Photoshop" => {
                    s.children = meta::read_photoshop_irb(payload, p + 4, &mut w.md);
                }
                "APP14/Adobe" if payload.len() >= 12 => {
                    s = s.d("transform", payload[11]);
                }
                _ => {}
            },
            0xDB => {
                let mut q = 0;
                while q < payload.len() {
                    let pq = payload[q] >> 4;
                    let tq = payload[q] & 15;
                    let n = if pq == 0 { 64 } else { 128 };
                    if pq > 1 || tq > 3 || q + 1 + n > payload.len() {
                        w.md.problems.push(format!(
                            "JPEG structure: invalid quantization table in DQT at offset {pos}"
                        ));
                        w.damaged = true;
                        break;
                    }
                    let mut values = vec![0u16; 64];
                    for (k, &nat) in ZIGZAG.iter().enumerate() {
                        values[nat] = if pq == 0 {
                            payload[q + 1 + k] as u16
                        } else {
                            be16(payload, q + 1 + 2 * k).unwrap_or(0)
                        };
                    }
                    w.info.qtables.push(QuantTable {
                        id: tq,
                        precision: pq,
                        values,
                    });
                    q += 1 + n;
                }
                let ids: Vec<String> = w.info.qtables.iter().map(|t| t.id.to_string()).collect();
                s = s.d("tables_so_far", ids.join(","));
            }
            0xC4 => {
                let mut q = 0;
                let mut ids = vec![];
                while q + 17 <= payload.len() {
                    let tc = payload[q] >> 4;
                    let th = payload[q] & 15;
                    let counts = payload[q + 1..q + 17].to_vec();
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    if tc > 1 || th > 3 || q + 17 + total > payload.len() {
                        w.md.problems.push(format!(
                            "JPEG structure: invalid Huffman table in DHT at offset {pos}"
                        ));
                        w.damaged = true;
                        break;
                    }
                    let t = HuffmanTable {
                        class: tc,
                        id: th,
                        counts,
                        symbols: payload[q + 17..q + 17 + total].to_vec(),
                    };
                    ids.push(format!("{}{}", if tc == 0 { "DC" } else { "AC" }, th));
                    w.info.huffman.push(t.clone());
                    w.events.push(Ev::Dht(t));
                    q += 17 + total;
                }
                s = s.d("tables", ids.join(","));
            }
            0xDD => {
                let ri = be16(payload, 0).unwrap_or(0);
                w.info.restart_interval = ri;
                w.events.push(Ev::Dri(ri));
                s = s.d("interval", ri);
            }
            0xFE => {
                let c = text(payload, 1000);
                w.md.tags.push(crate::model::Tag {
                    group: "COM".into(),
                    id: 0,
                    name: "Comment".into(),
                    value: c.clone(),
                });
                w.info.comments.push(c);
            }
            0xDA => {
                let ns = payload.first().copied().unwrap_or(0) as usize;
                let mut comps = vec![];
                for i in 0..ns {
                    if let (Some(&c), Some(&t)) = (payload.get(1 + 2 * i), payload.get(2 + 2 * i)) {
                        comps.push((c, t >> 4, t & 15));
                    }
                }
                let b = |i: usize| payload.get(1 + 2 * ns + i).copied().unwrap_or(0);
                let (ss, se, a) = (b(0), b(1), b(2));
                // entropy-coded data up to the next marker other than RSTn
                let start = end;
                let mut q = start;
                let mut rst = 0u32;
                while q < len {
                    match data[q..].iter().position(|&x| x == 0xFF) {
                        None => {
                            q = len;
                            break;
                        }
                        Some(i) => q += i,
                    }
                    if q + 1 >= len {
                        q = len;
                        break;
                    }
                    match data[q + 1] {
                        0x00 => q += 2,
                        0xFF => q += 1,
                        0xD0..=0xD7 => {
                            rst += 1;
                            q += 2
                        }
                        _ => break,
                    }
                }
                s = s.d("components", ns).d("spectral", format!("{ss}-{se}"));
                w.segs.push(s);
                let mut e = seg("ECS", start, q - start).d("rst_markers", rst);
                if q >= len {
                    e = e.d("truncated", "true");
                    w.md.problems.push(format!(
                        "JPEG structure: the entropy-coded data starting at offset {start} runs to the end of the file (no EOI)"
                    ));
                    w.damaged = true;
                }
                w.segs.push(e);
                w.events.push(Ev::Sos(Scan {
                    comps,
                    ss,
                    se,
                    ah: a >> 4,
                    al: a & 15,
                    ecs: (start, q),
                }));
                pos = q;
                continue;
            }
            _ if sof_name(m).is_some() => {
                if !w.info.sof.is_empty() {
                    w.md.problems.push(format!(
                        "JPEG structure: a second frame header ({kind}) at offset {pos}"
                    ));
                } else if payload.len() >= 6 {
                    w.info.sof = kind.clone();
                    w.info.precision = payload[0];
                    w.info.height = be16(payload, 1).unwrap_or(0) as u32;
                    w.info.width = be16(payload, 3).unwrap_or(0) as u32;
                    let nf = payload[5] as usize;
                    for i in 0..nf {
                        if let Some(c) = payload.get(6 + 3 * i..9 + 3 * i) {
                            w.info.components.push(Component {
                                id: c[0],
                                h: c[1] >> 4,
                                v: c[1] & 15,
                                tq: c[2],
                            });
                        }
                    }
                    s = s
                        .d("width", w.info.width)
                        .d("height", w.info.height)
                        .d("components", nf);
                }
            }
            _ => {}
        }
        w.segs.push(s);
        pos = end;
    }
    w.info.subsampling = if w.info.components.is_empty() {
        String::new()
    } else {
        subsampling(&w.info.components)
    };
    // ICC profile: chunks in sequence order
    if !icc.is_empty() {
        icc.sort_by_key(|c| c.0);
        let all: Vec<u8> = icc.iter().flat_map(|c| c.2.iter().copied()).collect();
        let count = icc[0].1 as usize;
        if count != icc.len() {
            w.md.problems.push(format!(
                "ICC profile: {} of {count} chunks present",
                icc.len()
            ));
        }
        w.md.icc_sha256 = Some(meta::sha256_hex(&all));
    }
    // after EOI
    if let Some(e) = w.eoi_end {
        w.info.after_eoi = (len - e) as u64;
        let mut p = e;
        let mut entries: Vec<&MpEntry> = mpf.iter().filter(|m| m.offset >= e).collect();
        entries.sort_by_key(|m| m.offset);
        for m in entries {
            if m.offset < p || m.offset >= len {
                continue;
            }
            let valid =
                data[m.offset..].starts_with(&[0xFF, 0xD8, 0xFF]) && m.offset + m.size <= len;
            if !valid {
                continue;
            }
            if m.offset > p {
                w.segs.push(seg("trailer", p, m.offset - p));
            }
            w.segs.push(seg("MPF image", m.offset, m.size));
            w.mpf_bytes += m.size as u64;
            p = m.offset + m.size;
        }
        if p < len {
            w.segs.push(seg("trailer", p, len - p));
        }
    }
    Ok(w)
}

/// Reads the MP Index IFD of an APP2/MPF segment. `tiff_abs` is the file offset of the MP header (offsets in
/// MP entries are relative to it; the first image has offset 0).
fn read_mpf(payload: &[u8], tiff_abs: usize, md: &mut Metadata) -> Vec<MpEntry> {
    let t = payload.get(4..).unwrap_or(&[]);
    let le = t.starts_with(b"II*\0");
    if !le && !t.starts_with(b"MM\0*") {
        md.problems.push("MPF: invalid header".into());
        return vec![];
    }
    let r16 = |o: usize| if le { super::le16(t, o) } else { be16(t, o) };
    let r32 = |o: usize| if le { super::le32(t, o) } else { be32(t, o) };
    let Some(ifd) = r32(4).map(|v| v as usize) else {
        return vec![];
    };
    let n = r16(ifd).unwrap_or(0) as usize;
    let mut out = vec![];
    for i in 0..n.min(64) {
        let e = ifd + 2 + 12 * i;
        let (Some(tag), Some(count), Some(off)) = (r16(e), r32(e + 4), r32(e + 8)) else {
            md.problems.push("MPF: index truncated".into());
            break;
        };
        if tag != 0xB002 {
            continue;
        }
        let off = off as usize;
        for k in 0..(count as usize / 16).min(64) {
            let b = off + 16 * k;
            let (Some(size), Some(o)) = (r32(b + 4), r32(b + 8)) else {
                md.problems.push("MPF: entry list truncated".into());
                break;
            };
            if o != 0 {
                out.push(MpEntry {
                    offset: tiff_abs + o as usize,
                    size: size as usize,
                });
            }
        }
    }
    out
}

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    let w = walk(data)?;
    let mut p = empty(Format::Jpeg);
    p.width = w.info.width;
    p.height = w.info.height;
    p.structure = w.segs;
    p.metadata = w.md;
    p.thumbnail = w.thumbnail;
    p.jpeg = Some(w.info);
    p.signals = super::signals::jpeg_structure(&p, w.mpf_bytes);
    Ok(p)
}

// ------------------------------------------------------------------------------------------------ decoder

struct Huff {
    /// (length << 8) | symbol for codes of up to 9 bits, indexed by the next 9 bits; 0 = longer code.
    fast: Vec<u16>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    symbols: Vec<u8>,
}

const FAST: u32 = 9;

impl Huff {
    fn new(t: &HuffmanTable) -> Result<Huff, String> {
        let mut h = Huff {
            fast: vec![0; 1 << FAST],
            maxcode: [-1; 18],
            valptr: [0; 17],
            mincode: [0; 17],
            symbols: t.symbols.clone(),
        };
        let mut code: i32 = 0;
        let mut k: i32 = 0;
        for l in 1..=16usize {
            let n = t.counts.get(l - 1).copied().unwrap_or(0) as i32;
            if n > 0 {
                h.valptr[l] = k;
                h.mincode[l] = code;
                code += n;
                k += n;
                if code > (1 << l) {
                    return Err("invalid Huffman table (code overflow)".into());
                }
                h.maxcode[l] = code - 1;
                if l as u32 <= FAST {
                    for c in h.mincode[l]..=h.maxcode[l] {
                        let sym = h.symbols.get((h.valptr[l] + c - h.mincode[l]) as usize);
                        let sym = *sym.ok_or("invalid Huffman table (too few symbols)")?;
                        let shift = FAST - l as u32;
                        let base = (c as usize) << shift;
                        for j in 0..(1usize << shift) {
                            h.fast[base + j] = ((l as u16) << 8) | sym as u16;
                        }
                    }
                }
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        if (k as usize) > h.symbols.len() {
            return Err("invalid Huffman table (too few symbols)".into());
        }
        Ok(h)
    }
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    end: usize,
    acc: u64,
    n: u32,
    /// Zero bits appended after a marker (not real data).
    phantom: u32,
    marker: bool,
    overrun: bool,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8], start: usize, end: usize) -> Self {
        Bits {
            d,
            pos: start,
            end,
            acc: 0,
            n: 0,
            phantom: 0,
            marker: false,
            overrun: false,
        }
    }
    #[inline]
    fn fill(&mut self) {
        while self.n <= 56 {
            let mut byte = 0u8;
            if !self.marker && self.pos < self.end {
                let b = self.d[self.pos];
                if b == 0xFF {
                    if self.pos + 1 < self.end && self.d[self.pos + 1] == 0x00 {
                        byte = 0xFF;
                        self.pos += 2;
                    } else {
                        self.marker = true;
                        self.phantom += 8;
                    }
                } else {
                    byte = b;
                    self.pos += 1;
                }
            } else {
                self.marker = true;
                self.phantom += 8;
            }
            self.acc |= (byte as u64) << (56 - self.n);
            self.n += 8;
        }
    }
    #[inline]
    fn consume(&mut self, k: u32) {
        self.acc <<= k;
        self.n -= k;
        if self.n < self.phantom {
            self.overrun = true;
            self.phantom = self.n;
        }
    }
    #[inline]
    fn get(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.fill();
        }
        let v = (self.acc >> (64 - k)) as u32;
        self.consume(k);
        v
    }
    #[inline]
    fn decode(&mut self, h: &Huff) -> Result<u8, String> {
        if self.n < 16 {
            self.fill();
        }
        let e = h.fast[(self.acc >> (64 - FAST)) as usize];
        if e != 0 {
            self.consume((e >> 8) as u32);
            return Ok(e as u8);
        }
        let mut l = FAST as usize + 1;
        let mut code = (self.acc >> (64 - l)) as i32;
        while l <= 16 && code > h.maxcode[l] {
            l += 1;
            code = (self.acc >> (64 - l)) as i32;
        }
        if l > 16 {
            return Err("invalid Huffman code in the entropy-coded data".into());
        }
        self.consume(l as u32);
        h.symbols
            .get((h.valptr[l] + code - h.mincode[l]) as usize)
            .copied()
            .ok_or_else(|| "invalid Huffman code in the entropy-coded data".into())
    }
    #[inline]
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.get(s) as i32;
        if v < (1 << (s - 1)) {
            v - (1 << s) + 1
        } else {
            v
        }
    }
    /// At a restart boundary: drop the padding bits, then expect RST(n % 8) (fill bytes allowed).
    fn restart(&mut self, n: u32) -> Result<(), String> {
        self.acc = 0;
        self.n = 0;
        self.phantom = 0;
        self.marker = false;
        let mut p = self.pos;
        while p < self.end && self.d[p] == 0xFF && self.d.get(p + 1) == Some(&0xFF) {
            p += 1;
        }
        let want = 0xD0 + (n % 8) as u8;
        if p + 2 <= self.end && self.d[p] == 0xFF && self.d[p + 1] == want {
            self.pos = p + 2;
            Ok(())
        } else {
            Err(format!(
                "restart marker RST{} expected at offset {}",
                n % 8,
                self.pos
            ))
        }
    }
}

/// Cap on coefficient blocks decoded (64 coefficients × 2 bytes each: 1 GiB).
const MAX_BLOCKS: usize = 8 << 20;

pub fn decode_coefficients(data: &[u8]) -> Result<Vec<ComponentCoefficients>, String> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err("unsupported: not a JPEG file".into());
    }
    let w = walk(data)?;
    let info = &w.info;
    match info.sof.as_str() {
        "SOF0" | "SOF1" => {}
        "" => return Err("no frame header (SOF) found".into()),
        "SOF2" | "SOF6" => return Err("unsupported: progressive".into()),
        "SOF3" | "SOF7" => return Err("unsupported: lossless".into()),
        "SOF5" => return Err("unsupported: hierarchical".into()),
        "SOF10" | "SOF14" => return Err("unsupported: progressive arithmetic coding".into()),
        _ => return Err("unsupported: arithmetic coding".into()),
    }
    if info.width == 0 || info.height == 0 {
        return Err("unsupported: zero image dimension (DNL)".into());
    }
    let comps = &info.components;
    if comps.is_empty() || comps.len() > 4 {
        return Err(format!("unsupported: {} components", comps.len()));
    }
    if comps
        .iter()
        .any(|c| !(1..=4).contains(&c.h) || !(1..=4).contains(&c.v))
    {
        return Err("invalid sampling factors".into());
    }
    let hmax = comps.iter().map(|c| c.h as usize).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v as usize).max().unwrap_or(1);
    let (x, y) = (info.width as usize, info.height as usize);
    let mcux = x.div_ceil(8 * hmax);
    let mcuy = y.div_ceil(8 * vmax);
    let dims: Vec<(usize, usize)> = comps
        .iter()
        .map(|c| (mcux * c.h as usize, mcuy * c.v as usize))
        .collect();
    let total: usize = dims.iter().map(|(a, b)| a * b).sum();
    // every coded block needs ≥ 2 bits (one DC code, one EOB): a cheap bound against hostile sizes
    let ecs_bytes: usize = w
        .events
        .iter()
        .map(|e| match e {
            Ev::Sos(s) => s.ecs.1 - s.ecs.0,
            _ => 0,
        })
        .sum();
    let coded: usize = comps
        .iter()
        .map(|c| {
            (x * c.h as usize).div_ceil(hmax).div_ceil(8)
                * (y * c.v as usize).div_ceil(vmax).div_ceil(8)
        })
        .sum();
    if total > MAX_BLOCKS {
        return Err(format!(
            "image too large for coefficient decoding ({total} blocks)"
        ));
    }
    if coded > ecs_bytes.saturating_mul(4) + 4 {
        return Err("entropy-coded data too short for the declared image size".into());
    }
    let mut out: Vec<ComponentCoefficients> = comps
        .iter()
        .zip(&dims)
        .map(|(c, &(bw, bh))| ComponentCoefficients {
            component_id: c.id,
            tq: c.tq,
            blocks_wide: bw as u32,
            blocks_high: bh as u32,
            coefs: vec![0i16; bw * bh * 64],
        })
        .collect();
    let mut dc_tabs: [Option<Huff>; 4] = [None, None, None, None];
    let mut ac_tabs: [Option<Huff>; 4] = [None, None, None, None];
    let mut ri = 0u16;
    let mut scans = 0;
    for ev in &w.events {
        match ev {
            Ev::Dht(t) => {
                let h = Huff::new(t)?;
                let slot = (t.id & 3) as usize;
                if t.class == 0 {
                    dc_tabs[slot] = Some(h);
                } else {
                    ac_tabs[slot] = Some(h);
                }
            }
            Ev::Dri(r) => ri = *r,
            Ev::Sos(s) => {
                scans += 1;
                if s.ss != 0 || s.se != 63 || s.ah != 0 || s.al != 0 {
                    return Err("invalid sequential scan parameters".into());
                }
                if s.comps.is_empty() || s.comps.len() > 4 {
                    return Err("invalid scan component count".into());
                }
                let mut idx = vec![];
                for &(cid, td, ta) in &s.comps {
                    let ci = comps
                        .iter()
                        .position(|c| c.id == cid)
                        .ok_or_else(|| format!("scan references unknown component {cid}"))?;
                    if dc_tabs[(td & 3) as usize].is_none() || ac_tabs[(ta & 3) as usize].is_none()
                    {
                        return Err("scan uses an undefined Huffman table".into());
                    }
                    idx.push((ci, (td & 3) as usize, (ta & 3) as usize));
                }
                let mut bits = Bits::new(data, s.ecs.0, s.ecs.1);
                let mut pred = vec![0i32; idx.len()];
                let mut mcu_count = 0usize;
                let mut rst_n = 0u32;
                // (blocks across, blocks down, blocks per MCU across/down) of this scan
                let (mx_n, my_n) = if idx.len() == 1 {
                    let c = &comps[idx[0].0];
                    (
                        (x * c.h as usize).div_ceil(hmax).div_ceil(8),
                        (y * c.v as usize).div_ceil(vmax).div_ceil(8),
                    )
                } else {
                    let per_mcu: usize = idx
                        .iter()
                        .map(|&(ci, _, _)| comps[ci].h as usize * comps[ci].v as usize)
                        .sum();
                    if per_mcu > 10 {
                        return Err("too many blocks per MCU".into());
                    }
                    (mcux, mcuy)
                };
                for my in 0..my_n {
                    for mx in 0..mx_n {
                        if ri > 0 && mcu_count > 0 && mcu_count.is_multiple_of(ri as usize) {
                            bits.restart(rst_n)?;
                            rst_n += 1;
                            pred.iter_mut().for_each(|p| *p = 0);
                        }
                        for (k, &(ci, td, ta)) in idx.iter().enumerate() {
                            let c = &comps[ci];
                            let (dc, ac) = (
                                dc_tabs[td].as_ref().ok_or("no DC table")?,
                                ac_tabs[ta].as_ref().ok_or("no AC table")?,
                            );
                            let (bh, bv) = if idx.len() == 1 {
                                (1, 1)
                            } else {
                                (c.h as usize, c.v as usize)
                            };
                            let o = &mut out[ci];
                            let stride = o.blocks_wide as usize;
                            for v in 0..bv {
                                for h in 0..bh {
                                    let (bx, by) = (mx * bh + h, my * bv + v);
                                    let base = (by * stride + bx) * 64;
                                    let blk = o
                                        .coefs
                                        .get_mut(base..base + 64)
                                        .ok_or("block outside the coefficient grid")?;
                                    decode_block(&mut bits, &mut pred[k], blk, dc, ac)?;
                                }
                            }
                        }
                        mcu_count += 1;
                        if bits.overrun {
                            return Err(format!(
                                "entropy-coded data of scan {scans} ends prematurely (MCU row {my}, column {mx})"
                            ));
                        }
                    }
                }
            }
        }
    }
    if scans == 0 {
        return Err("no scan (SOS) found".into());
    }
    Ok(out)
}

/// Decodes one 8×8 block (sequential Huffman) into `blk` (natural order).
#[inline]
fn decode_block(
    bits: &mut Bits,
    pred: &mut i32,
    blk: &mut [i16],
    dc: &Huff,
    ac: &Huff,
) -> Result<(), String> {
    let t = bits.decode(dc)? as u32;
    if t > 15 {
        return Err("invalid DC magnitude category".into());
    }
    *pred += bits.receive_extend(t);
    blk[0] = i16::try_from(*pred).map_err(|_| "DC coefficient out of range")?;
    let mut z = 1usize;
    while z < 64 {
        let rs = bits.decode(ac)?;
        let (r, sz) = ((rs >> 4) as usize, (rs & 15) as u32);
        if sz == 0 {
            if r == 15 {
                z += 16;
                continue;
            }
            break;
        }
        z += r;
        if z > 63 {
            return Err("AC coefficient index beyond 63".into());
        }
        blk[ZIGZAG[z]] = bits.receive_extend(sz) as i16;
        z += 1;
    }
    if z > 64 {
        return Err("AC run beyond the block".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a segment: marker + length + payload.
    fn segm(m: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, m];
        v.extend(((payload.len() + 2) as u16).to_be_bytes());
        v.extend(payload);
        v
    }

    /// Minimal 8×8 or 16×16 test JPEG with one-symbol Huffman tables: DC table {0: code "0", 1: "10"} and AC
    /// table {EOB: "0", 0x01: "10"}. `ecs` is the raw entropy-coded data.
    fn tiny(
        w: u16,
        h: u16,
        comps: &[(u8, u8, u8)],
        ri: Option<u16>,
        scans: &[(&[u8], Vec<u8>)],
    ) -> Vec<u8> {
        let mut f = vec![0xFF, 0xD8];
        let mut q = vec![0u8];
        q.extend((1..=64).map(|i| i as u8));
        f.extend(segm(0xDB, &q));
        let mut sof = vec![8];
        sof.extend(h.to_be_bytes());
        sof.extend(w.to_be_bytes());
        sof.push(comps.len() as u8);
        for &(id, hv, tq) in comps {
            sof.extend([id, hv, tq]);
        }
        f.extend(segm(0xC0, &sof));
        // DC: 1 code of length 1 (sym 0), 1 code of length 2 (sym 1)
        let mut dht = vec![0x00, 1, 1];
        dht.extend([0u8; 14]);
        dht.extend([0, 1]);
        // AC: EOB (0x00) length 1, 0x01 length 2
        dht.push(0x10);
        dht.extend([1, 1]);
        dht.extend([0u8; 14]);
        dht.extend([0x00, 0x01]);
        f.extend(segm(0xC4, &dht));
        if let Some(r) = ri {
            f.extend(segm(0xDD, &r.to_be_bytes()));
        }
        for (ids, ecs) in scans {
            let mut sos = vec![ids.len() as u8];
            for &id in ids.iter() {
                sos.extend([id, 0x00]);
            }
            sos.extend([0, 63, 0]);
            f.extend(segm(0xDA, &sos));
            f.extend(ecs);
        }
        f.extend([0xFF, 0xD9]);
        f
    }

    /// Bits → bytes, padded with 1s.
    fn pack(bits: &str) -> Vec<u8> {
        let mut v = vec![];
        let mut cur = 0u8;
        let mut n = 0;
        for c in bits.chars().filter(|c| *c == '0' || *c == '1') {
            cur = (cur << 1) | (c == '1') as u8;
            n += 1;
            if n == 8 {
                v.push(cur);
                if cur == 0xFF {
                    v.push(0);
                }
                cur = 0;
                n = 0;
            }
        }
        if n > 0 {
            cur = (cur << (8 - n)) | ((1u8 << (8 - n)) - 1);
            v.push(cur);
            if cur == 0xFF {
                v.push(0);
            }
        }
        v
    }

    #[test]
    fn zigzag_is_a_permutation() {
        let mut s = ZIGZAG.to_vec();
        s.sort();
        assert_eq!(s, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn grey_8x8_one_block() {
        // DC: category 1 ("10") value bit "1" → +1; AC: 0x01 ("10") value "0" → -1 at zz 1; EOB "0"
        let ecs = pack("10 1 10 0 0");
        let f = tiny(8, 8, &[(1, 0x11, 0)], None, &[(&[1], ecs)]);
        let p = parse(&f).unwrap();
        assert_eq!((p.width, p.height), (8, 8));
        let j = p.jpeg.unwrap();
        assert_eq!(j.markers, ["SOI", "DQT", "SOF0", "DHT", "SOS", "EOI"]);
        assert_eq!(j.subsampling, "grey");
        assert_eq!(j.qtables[0].values[1], 2); // zz 1 → natural 1
        assert_eq!(j.qtables[0].values[8], 3); // zz 2 → natural 8
        let c = decode_coefficients(&f).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].blocks_wide, c[0].blocks_high), (1, 1));
        assert_eq!(c[0].coefs[0], 1);
        assert_eq!(c[0].coefs[1], -1);
        assert!(c[0].coefs[2..].iter().all(|&v| v == 0));
        // structure covers the file
        let end = p
            .structure
            .iter()
            .map(|s| s.offset + s.length)
            .max()
            .unwrap();
        assert_eq!(end as usize, f.len());
    }

    #[test]
    fn byte_stuffing_is_removed() {
        // 8 blocks of 16x8? Use one block with DC category bits forming 0xFF: DC "10" + "1", AC "10" "1" then EOB...
        // Simplest: bits "1111 1111" appear as DC=cat1 (10)… craft: "10 1 10 1 10 1 10 1 0" → 0xB6 0xD…; instead
        // check the bit reader directly on stuffed bytes.
        let d = [0xFF, 0x00, 0x12, 0xFF, 0xD9];
        let mut b = Bits::new(&d, 0, 3);
        assert_eq!(b.get(8), 0xFF);
        assert_eq!(b.get(8), 0x12);
        assert!(!b.overrun);
        b.get(1);
        assert!(b.overrun);
    }

    #[test]
    fn restart_markers_reset_prediction() {
        // 16x8 grey: two blocks, restart interval 1. Each block: DC +1 then EOB. With RST the second DC is +1
        // again (prediction reset) instead of 2.
        let mut ecs = pack("10 1 0");
        ecs.extend([0xFF, 0xD0]);
        ecs.extend(pack("10 1 0"));
        let f = tiny(16, 8, &[(1, 0x11, 0)], Some(1), &[(&[1], ecs)]);
        let c = decode_coefficients(&f).unwrap();
        assert_eq!(c[0].coefs[0], 1);
        assert_eq!(c[0].coefs[64], 1);
        let ecs_seg = parse(&f)
            .unwrap()
            .structure
            .into_iter()
            .find(|s| s.kind == "ECS")
            .unwrap();
        assert_eq!(ecs_seg.detail["rst_markers"], "1");
        // missing RST → error
        let mut ecs = pack("10 1 0");
        ecs.extend(pack("10 1 0"));
        let f = tiny(16, 8, &[(1, 0x11, 0)], Some(1), &[(&[1], ecs)]);
        assert!(
            decode_coefficients(&f)
                .unwrap_err()
                .contains("restart marker")
        );
    }

    #[test]
    fn chroma_420_padding_blocks() {
        // 24x8 image, Y 2x2, Cb/Cr 1x1: MCU 16x16 → 2 MCUs across, 1 down. Y grid 4x2 blocks, C grid 2x1.
        // Each MCU = 4 Y blocks + Cb + Cr, all DC 0 ("0") and EOB ("0").
        let ecs = pack(&"0 0 ".repeat(12));
        let f = tiny(
            24,
            8,
            &[(1, 0x22, 0), (2, 0x11, 0), (3, 0x11, 0)],
            None,
            &[(&[1, 2, 3], ecs)],
        );
        let p = parse(&f).unwrap();
        assert_eq!(p.jpeg.as_ref().unwrap().subsampling, "4:2:0");
        let c = decode_coefficients(&f).unwrap();
        assert_eq!((c[0].blocks_wide, c[0].blocks_high), (4, 2));
        assert_eq!((c[1].blocks_wide, c[1].blocks_high), (2, 1));
        assert_eq!(c[0].coefs.len(), 4 * 2 * 64);
    }

    #[test]
    fn non_interleaved_scans_code_only_needed_blocks() {
        // 24x8 4:2:0, three separate scans. Y needs ceil(24/8)=3 x ceil(8/8)=1 blocks; Cb/Cr ceil(12/8)=2 x 1.
        // Y blocks get DC +1 each (pred accumulates: 1, 2, 3).
        let y = pack(&"10 1 0 ".repeat(3));
        let cb = pack(&"0 0 ".repeat(2));
        let cr = pack(&"0 0 ".repeat(2));
        let f = tiny(
            24,
            8,
            &[(1, 0x22, 0), (2, 0x11, 0), (3, 0x11, 0)],
            None,
            &[(&[1], y), (&[2], cb), (&[3], cr)],
        );
        let c = decode_coefficients(&f).unwrap();
        assert_eq!(c[0].coefs[0], 1);
        assert_eq!(c[0].coefs[64], 2);
        assert_eq!(c[0].coefs[128], 3);
        assert_eq!(c[0].coefs[192], 0); // padding block never coded
    }

    #[test]
    fn premature_end_is_an_error() {
        let ecs = pack("10"); // DC category but value bit missing → padding 1s are read, then AC runs out
        let f = tiny(16, 16, &[(1, 0x11, 0)], None, &[(&[1], ecs)]);
        assert!(decode_coefficients(&f).is_err());
    }

    #[test]
    fn progressive_and_arithmetic_unsupported() {
        let mut f = tiny(8, 8, &[(1, 0x11, 0)], None, &[(&[1], pack("0 0"))]);
        let i = f.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        f[i + 1] = 0xC2;
        assert_eq!(
            decode_coefficients(&f).unwrap_err(),
            "unsupported: progressive"
        );
        let p = parse(&f).unwrap();
        assert!(p.signals.iter().any(|s| s.code == "PROGRESSIVE_ENCODING"));
        f[i + 1] = 0xC9;
        assert!(decode_coefficients(&f).unwrap_err().contains("arithmetic"));
    }

    #[test]
    fn zero_length_and_truncated_segments() {
        // zero length segment
        let mut f = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x00, 1, 2, 3];
        let p = parse(&f).unwrap();
        assert!(!p.metadata.problems.is_empty());
        let end: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(end as usize, f.len());
        // huge declared length
        f = vec![0xFF, 0xD8, 0xFF, 0xDB, 0xFF, 0xFF, 0, 1, 2];
        let p = parse(&f).unwrap();
        assert_eq!(p.structure.last().unwrap().detail["truncated"], "true");
        assert!(decode_coefficients(&f).is_err());
        // just SOI
        let p = parse(&[0xFF, 0xD8]).unwrap();
        assert_eq!(p.width, 0);
        // not a JPEG
        assert!(parse(b"\xFF\xD9").is_err());
    }

    #[test]
    fn fill_bytes_and_garbage_between_segments() {
        let mut f = vec![0xFF, 0xD8, 0xFF, 0xFF, 0xFF];
        f.extend(segm(0xFE, b"hello"));
        f.extend([1, 2, 3]);
        f.extend([0xFF, 0xD9]);
        let p = parse(&f).unwrap();
        let kinds: Vec<&str> = p.structure.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["SOI", "COM", "garbage", "EOI"]);
        assert_eq!(p.jpeg.unwrap().comments, ["hello"]);
        let total: u64 = p.structure.iter().map(|s| s.length).sum();
        assert_eq!(total as usize, f.len());
    }

    #[test]
    fn huffman_table_validation() {
        // 3 codes of length 1 → overflow
        let t = HuffmanTable {
            class: 0,
            id: 0,
            counts: [vec![3u8], vec![0; 15]].concat(),
            symbols: vec![0, 1, 2],
        };
        assert!(Huff::new(&t).is_err());
        let t = HuffmanTable {
            class: 0,
            id: 0,
            counts: [vec![0u8, 2], vec![0; 14]].concat(),
            symbols: vec![0],
        };
        assert!(Huff::new(&t).is_err());
    }

    #[test]
    fn mpf_secondary_image_after_eoi() {
        // APP2/MPF with one secondary image entry pointing after EOI.
        let base = {
            let mut f = vec![0xFF, 0xD8];
            f.extend(segm(0xE2, &[0u8; 4 + 8 + 2 + 12 + 4 + 32])); // placeholder, patched below
            f.extend([0xFF, 0xD9]);
            f
        };
        let eoi_end = base.len();
        let second = [0xFF, 0xD8, 0xFF, 0xD9];
        // build MP header (big endian)
        let mut t = b"MM\0*".to_vec();
        t.extend(8u32.to_be_bytes());
        t.extend(1u16.to_be_bytes());
        t.extend(0xB002u16.to_be_bytes());
        t.extend(7u16.to_be_bytes());
        t.extend(32u32.to_be_bytes());
        t.extend(26u32.to_be_bytes()); // offset of entries
        t.extend(0u32.to_be_bytes());
        // entry 1 (primary): offset 0
        t.extend([0u8; 16]);
        // entry 2
        let tiff_abs = 2 + 4 + 4; // SOI + marker/len + "MPF\0"
        t.extend(0u32.to_be_bytes());
        t.extend((second.len() as u32).to_be_bytes());
        t.extend(((eoi_end - tiff_abs) as u32).to_be_bytes());
        t.extend([0u8; 4]);
        let mut payload = b"MPF\0".to_vec();
        payload.extend(&t);
        let mut f = vec![0xFF, 0xD8];
        f.extend(segm(0xE2, &payload));
        f.extend([0xFF, 0xD9]);
        assert_eq!(f.len(), eoi_end);
        f.extend(second);
        f.extend(b"junk");
        let p = parse(&f).unwrap();
        let kinds: Vec<&str> = p.structure.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["SOI", "APP2/MPF", "EOI", "MPF image", "trailer"]);
        let s = p
            .signals
            .iter()
            .find(|s| s.code == "DATA_AFTER_END_MARKER")
            .unwrap();
        assert_eq!(s.params["bytes"], "8");
        assert_eq!(s.params["mpf_bytes"], "4");
    }
}
