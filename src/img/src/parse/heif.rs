//! HEIF/HEIC (ISO/IEC 23008-12, ISOBMFF) box walker, structure only: ftyp, meta (hdlr, pitm, iinf/infe v2/v3,
//! iloc v0–2, iref, iprp/ipco/ipma with ispe/irot/imir/colr/hvcC/pixi/auxC, idat, dinf/dref), mdat. Items are
//! resolved to file extents; the grid descriptor, the Exif item (4-byte TIFF-header offset prefix), XMP mime
//! items and a JPEG thumbnail item are read. Pixels are never decoded.
//!
//! `unreferenced_bytes` = bytes of mdat/idat payloads covered by no item extent, plus bytes after the last
//! top-level box. Box headers and the fields of every other box count as referenced; free/skip payloads are
//! padding and are not counted.

use super::{SegExt, be16, be32, be64, empty, fourcc, meta, seg, signals::sig, text};
use crate::model::{Format, HeifInfo, HeifItem, Metadata, ParsedImage, Segment};

const CONTAINERS: &[&str] = &[
    "meta", "iinf", "iprp", "ipco", "iref", "dinf", "dref", "moov", "trak", "mdia", "minf", "stbl",
    "grpl",
];
const MAX_DEPTH: usize = 12;
const MAX_BOXES: usize = 20_000;

/// (item id, construction method, data reference index, base offset, extents)
type IlocEntry = (u32, u8, u16, u64, Vec<(u64, u64)>);

#[derive(Default)]
struct Ctx {
    boxes: usize,
    info: HeifInfo,
    md: Metadata,
    problems: Vec<String>,
    hdlr: Option<String>,
    pitm: Option<u32>,
    /// (item id, construction method, data ref index, base offset, extents (offset, length) relative)
    iloc: Vec<IlocEntry>,
    idat: Option<(usize, usize)>,
    /// Payload ranges of mdat and idat boxes.
    holes: Vec<(usize, usize)>,
    /// Property boxes of ipco in order: (type, payload start, payload end).
    props: Vec<(String, usize, usize)>,
    /// item id → property indices (1-based).
    ipma: Vec<(u32, Vec<u16>)>,
    content_types: Vec<(u32, String)>,
    external_dref: bool,
    ftyp_first: bool,
}

/// Box header at `pos` within `end`: (type, header length, box end, truncated).
fn header(d: &[u8], pos: usize, end: usize) -> Option<(String, usize, usize, bool)> {
    let size = be32(d, pos)? as u64;
    let ty = d.get(pos + 4..pos + 8)?;
    let mut hl = 8usize;
    let size = match size {
        1 => {
            hl = 16;
            be64(d, pos + 8)?
        }
        0 => (end - pos) as u64,
        s => s,
    };
    if ty == b"uuid" {
        hl += 16;
    }
    if size < hl as u64 {
        return None;
    }
    let want = (pos as u64).checked_add(size)?;
    let truncated = want > end as u64;
    let bend = if truncated { end } else { want as usize };
    if pos + hl > bend {
        return None;
    }
    Some((fourcc(ty), hl, bend, truncated))
}

fn walk(d: &[u8], start: usize, end: usize, path: &str, depth: usize, c: &mut Ctx) -> Vec<Segment> {
    let mut out = vec![];
    let mut pos = start;
    while pos < end {
        c.boxes += 1;
        if c.boxes > MAX_BOXES {
            c.problems.push("too many boxes".into());
            out.push(seg("unparsed", pos, end - pos));
            break;
        }
        let Some((ty, hl, bend, truncated)) = header(d, pos, end) else {
            let kind = if path.is_empty() {
                "trailer".to_string()
            } else {
                format!("{path}/garbage")
            };
            out.push(seg(&kind, pos, end - pos));
            if path.is_empty() {
                // bytes after the last box: counted as unreferenced
                c.holes.push((pos, end));
            } else {
                c.md.problems.push(format!(
                    "HEIF structure: {} unparseable byte(s) at offset {pos} inside {path}",
                    end - pos
                ));
            }
            break;
        };
        let name = if path.is_empty() {
            ty.clone()
        } else {
            format!("{path}/{ty}")
        };
        let mut s = seg(&name, pos, bend - pos);
        if truncated {
            s = s.d("truncated", "true");
            c.md.problems.push(format!(
                "HEIF structure: box {ty} at offset {pos} runs past the end of its parent (file truncated)"
            ));
        }
        if path.is_empty() && out.is_empty() {
            c.ftyp_first = ty == "ftyp";
        }
        let p0 = pos + hl;
        let body = &d[p0..bend];
        let full = |n: usize| -> (u8, u32, usize) {
            let v = body.first().copied().unwrap_or(0);
            let f = be32(body, 0).unwrap_or(0) & 0xFF_FFFF;
            (v, f, p0 + n)
        };
        if depth < MAX_DEPTH && CONTAINERS.contains(&ty.as_str()) {
            let (v, _, mut cs) = full(4);
            match ty.as_str() {
                "iinf" => cs += if v == 0 { 2 } else { 4 },
                "iref" => {
                    let (v, _, _) = full(4);
                    read_iref(d, p0 + 4, bend, v, c);
                    s.children = vec![];
                    out.push(s);
                    pos = bend;
                    continue;
                }
                "dref" => cs += 4,
                "meta" => {
                    // QuickTime-style meta without version/flags
                    if be32(body, 4).is_some_and(|_| body.get(4..8) == Some(b"hdlr")) {
                        cs = p0;
                    }
                }
                "iprp" | "ipco" | "dinf" | "moov" | "trak" | "mdia" | "minf" | "stbl" | "grpl" => {
                    cs = p0
                }
                _ => {}
            }
            let cs = cs.min(bend);
            if ty == "ipco" {
                // remember property boxes in order
                let mut q = cs;
                while q < bend {
                    match header(d, q, bend) {
                        Some((pt, phl, pend, _)) => {
                            c.props.push((pt, q + phl, pend));
                            q = pend;
                        }
                        None => break,
                    }
                }
            }
            s.children = walk(d, cs, bend, &name, depth + 1, c);
            if name.ends_with("dref") {
                for ch in &s.children {
                    if ch.kind.ends_with("url ") || ch.kind.ends_with("urn ") {
                        let o = ch.offset as usize + 8;
                        if be32(d, o).is_some_and(|f| f & 1 == 0) {
                            c.external_dref = true;
                        }
                    }
                }
            }
        } else {
            match ty.as_str() {
                "ftyp" if body.len() >= 8 => {
                    c.info.major_brand = fourcc(&body[0..4]);
                    c.info.compatible_brands = body[8..].chunks_exact(4).map(fourcc).collect();
                    s = s
                        .d("brand", &c.info.major_brand)
                        .d("compatible", c.info.compatible_brands.join(","));
                }
                "hdlr" if body.len() >= 12 => {
                    let h = fourcc(&body[8..12]);
                    s = s.d("handler", &h);
                    if path.ends_with("meta") && c.hdlr.is_none() {
                        c.hdlr = Some(h);
                    }
                }
                "pitm" => {
                    let (v, _, _) = full(4);
                    let id = if v == 0 {
                        be16(body, 4).map(u32::from)
                    } else {
                        be32(body, 4)
                    };
                    if let Some(id) = id {
                        c.pitm = Some(id);
                        s = s.d("item", id);
                    }
                }
                "infe" => {
                    if let Some(it) = read_infe(body, c) {
                        s = s.d("id", it.id).d("type", &it.item_type);
                        if it.hidden {
                            s = s.d("hidden", "true");
                        }
                        c.info.items.push(it);
                    }
                }
                "iloc" => read_iloc(body, c),
                "ipma" => read_ipma(body, c),
                "idat" => {
                    c.idat = Some((p0, bend));
                    c.holes.push((p0, bend));
                }
                "mdat" => c.holes.push((p0, bend)),
                "free" | "skip" => s = s.d("padding", bend - p0),
                _ => {}
            }
        }
        out.push(s);
        pos = bend;
    }
    out
}

fn read_infe(b: &[u8], c: &mut Ctx) -> Option<HeifItem> {
    let v = *b.first()?;
    let flags = be32(b, 0)? & 0xFF_FFFF;
    let (id, mut q) = match v {
        0 | 1 => (be16(b, 4)? as u32, 8),
        2 => (be16(b, 4)? as u32, 8),
        _ => (be32(b, 4)?, 10),
    };
    let item_type = if v >= 2 {
        let t = fourcc(b.get(q..q + 4)?);
        q += 4;
        t
    } else {
        "mime".into()
    };
    // name
    let rest = b.get(q..)?;
    let nl = rest.iter().position(|&x| x == 0).unwrap_or(rest.len());
    q += nl + 1;
    if (item_type == "mime" || v < 2)
        && let Some(r) = b.get(q..)
    {
        let cl = r.iter().position(|&x| x == 0).unwrap_or(r.len());
        c.content_types.push((id, text(&r[..cl], 120)));
    }
    Some(HeifItem {
        id,
        item_type,
        hidden: flags & 1 == 1,
        ..Default::default()
    })
}

fn read_iref(d: &[u8], start: usize, end: usize, v: u8, c: &mut Ctx) {
    let mut q = start;
    let w = if v == 0 { 2 } else { 4 };
    let rd = |o: usize| {
        if v == 0 {
            be16(d, o).map(u32::from)
        } else {
            be32(d, o)
        }
    };
    while q < end {
        let Some((ty, hl, bend, _)) = header(d, q, end) else {
            break;
        };
        let p = q + hl;
        if let (Some(from), Some(n)) = (rd(p), be16(d, p + w)) {
            for k in 0..n as usize {
                if let Some(to) = rd(p + w + 2 + k * w).filter(|_| p + w + 2 + (k + 1) * w <= bend)
                {
                    if let Some(it) = c.info.items.iter_mut().find(|i| i.id == from) {
                        it.refs.push((ty.clone(), to));
                    } else {
                        c.problems.push(format!(
                            "an item reference ({ty}) comes from item {from}, which is not declared"
                        ));
                    }
                }
            }
        }
        q = bend;
    }
}

fn read_iloc(b: &[u8], c: &mut Ctx) {
    let Some(&v) = b.first() else { return };
    if v > 2 {
        c.problems
            .push(format!("iloc version {v} is not supported"));
        return;
    }
    let (Some(&s1), Some(&s2)) = (b.get(4), b.get(5)) else {
        return;
    };
    let (os, ls, bs, is) = (
        (s1 >> 4) as usize,
        (s1 & 15) as usize,
        (s2 >> 4) as usize,
        (s2 & 15) as usize,
    );
    let is = if v == 0 { 0 } else { is };
    let rn = |o: usize, n: usize| -> Option<u64> {
        match n {
            0 => Some(0),
            4 => be32(b, o).map(u64::from),
            8 => be64(b, o),
            _ => None,
        }
    };
    let mut q = 6;
    let count = if v < 2 {
        q += 2;
        be16(b, 6).map(u32::from)
    } else {
        q += 4;
        be32(b, 6)
    };
    let Some(count) = count else { return };
    for _ in 0..count.min(100_000) {
        let id = if v < 2 {
            q += 2;
            be16(b, q - 2).map(u32::from)
        } else {
            q += 4;
            be32(b, q - 4)
        };
        let Some(id) = id else { break };
        let mut method = 0u8;
        if v >= 1 {
            let Some(m) = be16(b, q) else { break };
            method = (m & 15) as u8;
            q += 2;
        }
        let Some(dri) = be16(b, q) else { break };
        q += 2;
        let Some(base) = rn(q, bs) else { break };
        q += bs;
        let Some(n) = be16(b, q) else { break };
        q += 2;
        let mut ext = vec![];
        for _ in 0..n {
            q += is;
            let (Some(o), Some(l)) = (rn(q, os), rn(q + os, ls)) else {
                c.problems.push("the iloc box is truncated".into());
                c.iloc.push((id, method, dri, base, ext));
                return;
            };
            q += os + ls;
            ext.push((o, l));
        }
        c.iloc.push((id, method, dri, base, ext));
    }
}

fn read_ipma(b: &[u8], c: &mut Ctx) {
    let Some(&v) = b.first() else { return };
    let flags = be32(b, 0).unwrap_or(0) & 0xFF_FFFF;
    let Some(n) = be32(b, 4) else { return };
    let mut q = 8;
    for _ in 0..n.min(100_000) {
        let id = if v < 1 {
            q += 2;
            be16(b, q - 2).map(u32::from)
        } else {
            q += 4;
            be32(b, q - 4)
        };
        let (Some(id), Some(&k)) = (id, b.get(q)) else {
            break;
        };
        q += 1;
        let mut idx = vec![];
        for _ in 0..k {
            if flags & 1 == 1 {
                let Some(x) = be16(b, q) else { break };
                idx.push(x & 0x7FFF);
                q += 2;
            } else {
                let Some(&x) = b.get(q) else { break };
                idx.push((x & 0x7F) as u16);
                q += 1;
            }
        }
        c.ipma.push((id, idx));
    }
}

fn item_data(d: &[u8], it: &HeifItem) -> Vec<u8> {
    let mut v = vec![];
    for &(o, l) in &it.extents {
        let (o, l) = (o as usize, l as usize);
        if let Some(s) = d.get(o..o.saturating_add(l).min(d.len())) {
            v.extend_from_slice(s);
        }
        if v.len() > 64 << 20 {
            break;
        }
    }
    v
}

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    if Format::sniff(data) != Format::Heif {
        return Err("not a HEIF file".into());
    }
    let mut c = Ctx::default();
    let structure = walk(data, 0, data.len(), "", 0, &mut c);
    let len = data.len() as u64;
    // resolve extents
    let iloc = std::mem::take(&mut c.iloc);
    for (id, method, dri, base, ext) in iloc {
        if dri != 0 {
            c.external_dref = true;
        }
        let origin = match method {
            0 => 0u64,
            1 => match c.idat {
                Some((s, _)) => s as u64,
                None => {
                    c.problems.push(format!(
                        "item {id} is stored in idat but there is no idat box"
                    ));
                    continue;
                }
            },
            _ => {
                c.problems.push(format!(
                    "item {id} uses iloc construction method {method} (not resolved)"
                ));
                continue;
            }
        };
        let abs: Vec<(u64, u64)> = ext
            .iter()
            .map(|&(o, l)| {
                let off = origin.saturating_add(base).saturating_add(o);
                let l = if l == 0 { len.saturating_sub(off) } else { l };
                (off, l)
            })
            .collect();
        for &(o, l) in &abs {
            if o.saturating_add(l) > len {
                c.problems.push(format!("an extent of item {id} ({l} bytes at offset {o}) lies beyond the end of the file"));
            }
        }
        match c.info.items.iter_mut().find(|i| i.id == id) {
            Some(it) => it.extents = abs,
            None => c.problems.push(format!(
                "iloc locates item {id}, which is not declared in iinf"
            )),
        }
    }
    // properties
    let props = std::mem::take(&mut c.props);
    let ipma = std::mem::take(&mut c.ipma);
    let mut icc: Option<Vec<u8>> = None;
    for (id, idx) in &ipma {
        let Some(it) = c.info.items.iter_mut().find(|i| i.id == *id) else {
            continue;
        };
        for &k in idx {
            let Some((ty, s, e)) = (k as usize).checked_sub(1).and_then(|k| props.get(k)) else {
                continue;
            };
            let b = &data[*s..*e];
            match ty.as_str() {
                "ispe" => {
                    it.width = be32(b, 4).unwrap_or(0);
                    it.height = be32(b, 8).unwrap_or(0);
                }
                "colr"
                    if icc.is_none()
                        && b.len() >= 4
                        && (&b[..4] == b"prof" || &b[..4] == b"rICC") =>
                {
                    icc = Some(b[4..].to_vec());
                }
                _ => {}
            }
        }
    }
    if let Some(i) = icc {
        c.md.icc_sha256 = Some(meta::sha256_hex(&i));
    }
    // primary, grid
    c.info.primary_item = c.pitm.unwrap_or(0);
    let primary = c.info.items.iter().find(|i| Some(i.id) == c.pitm).cloned();
    match (&c.pitm, &primary) {
        (None, _) => c.problems.push("there is no primary item (pitm)".into()),
        (Some(id), None) => c
            .problems
            .push(format!("the primary item {id} is not declared")),
        (Some(_), Some(p)) if p.hidden => {
            c.problems.push("the primary item is marked hidden".into())
        }
        _ => {}
    }
    if c.hdlr.as_deref().is_some_and(|h| h != "pict") {
        c.problems.push(format!(
            "the meta handler is \"{}\" instead of \"pict\"",
            c.hdlr.clone().unwrap_or_default()
        ));
    } else if c.hdlr.is_none() {
        c.problems.push("the meta box has no hdlr".into());
    }
    if !c.ftyp_first {
        c.problems.push("the first box is not ftyp".into());
    }
    if c.external_dref {
        c.problems
            .push("data references point outside the file (dref)".into());
    }
    let (mut w, mut h) = primary
        .as_ref()
        .map(|p| (p.width, p.height))
        .unwrap_or((0, 0));
    if let Some(p) = &primary
        && p.item_type == "grid"
    {
        let g = item_data(data, p);
        if g.len() >= 8 {
            let rows = g[2] as u32 + 1;
            let cols = g[3] as u32 + 1;
            let (ow, oh) = if g[1] & 1 == 0 {
                (
                    be16(&g, 4).unwrap_or(0) as u32,
                    be16(&g, 6).unwrap_or(0) as u32,
                )
            } else {
                (be32(&g, 4).unwrap_or(0), be32(&g, 8).unwrap_or(0))
            };
            c.info.grid = Some((rows, cols, ow, oh));
            if w == 0 {
                (w, h) = (ow, oh);
            } else if (w, h) != (ow, oh) {
                c.problems.push(format!(
                    "the grid output is {ow}×{oh} but the primary item's ispe says {w}×{h}"
                ));
            }
            let tiles: Vec<u32> = p
                .refs
                .iter()
                .filter(|r| r.0 == "dimg")
                .map(|r| r.1)
                .collect();
            if tiles.len() as u32 != rows * cols {
                c.problems.push(format!(
                    "the grid has {rows}×{cols} cells but references {} tiles",
                    tiles.len()
                ));
            }
            let sizes: Vec<(u32, u32)> = tiles
                .iter()
                .filter_map(|t| c.info.items.iter().find(|i| i.id == *t))
                .map(|i| (i.width, i.height))
                .collect();
            if let Some(&(tw, th)) = sizes.first() {
                let odd: Vec<String> = sizes
                    .iter()
                    .zip(&tiles)
                    .filter(|(s, _)| **s != (tw, th))
                    .map(|(s, t)| format!("item {t} is {}×{}", s.0, s.1))
                    .collect();
                if !odd.is_empty() {
                    c.problems.push(format!(
                        "grid tiles differ in size (first tile {tw}×{th}; {})",
                        odd.join(", ")
                    ));
                }
                let (r, cl, tw6, th6, ow6, oh6) = (
                    rows as u64,
                    cols as u64,
                    tw as u64,
                    th as u64,
                    ow as u64,
                    oh as u64,
                );
                if tw > 0
                    && th > 0
                    && (cl * tw6 < ow6
                        || (cl - 1) * tw6 >= ow6
                        || r * th6 < oh6
                        || (r - 1) * th6 >= oh6)
                {
                    c.problems.push(format!(
                        "{rows}×{cols} tiles of {tw}×{th} do not fit the {ow}×{oh} grid output"
                    ));
                }
            }
        } else {
            c.problems
                .push("the grid item's data could not be read".into());
        }
    }
    c.info.width = w;
    c.info.height = h;
    // overlapping extents between items
    let mut all: Vec<(u64, u64, u32)> = c
        .info
        .items
        .iter()
        .flat_map(|i| {
            i.extents
                .iter()
                .map(move |&(o, l)| (o, o.saturating_add(l), i.id))
        })
        .collect();
    all.sort();
    let mut overl = vec![];
    for k in 1..all.len() {
        let (_, e0, i0) = all[k - 1];
        let (s1, _, i1) = all[k];
        if s1 < e0 && i0 != i1 {
            overl.push(format!("{i0}/{i1}"));
        }
    }
    if !overl.is_empty() {
        overl.dedup();
        c.problems
            .push(format!("item extents overlap (items {})", overl.join(", ")));
    }
    // metadata items and thumbnail
    let mut thumb: Option<Vec<u8>> = None;
    let items = c.info.items.clone();
    for it in &items {
        let ct = c
            .content_types
            .iter()
            .find(|x| x.0 == it.id)
            .map(|x| x.1.clone())
            .unwrap_or_default();
        if it.item_type == "Exif" {
            let b = item_data(data, it);
            match be32(&b, 0) {
                Some(off) if (4 + off as usize) < b.len() => {
                    let t = meta::read_exif(&b[4 + off as usize..], &mut c.md, "HEIF Exif item");
                    if thumb.is_none() {
                        thumb = t;
                    }
                }
                _ => c.md.problems.push(format!(
                    "HEIF: the Exif item {} is too short or its offset is invalid",
                    it.id
                )),
            }
        } else if it.item_type == "mime" && ct.contains("rdf+xml") {
            meta::read_xmp(&String::from_utf8_lossy(&item_data(data, it)), &mut c.md);
        }
        if let Some(p) = &primary
            && it.refs.iter().any(|r| r.0 == "thmb" && r.1 == p.id)
        {
            if it.width > 0 && it.height > 0 && p.width > 0 && p.height > 0 {
                let a = it.width as f64 / it.height as f64;
                let b = w as f64 / h.max(1) as f64;
                if ((a - b) / b).abs() > 0.02 {
                    c.problems.push(format!(
                        "the thumbnail item {} is {}×{}, a different aspect ratio from the {w}×{h} primary image",
                        it.id, it.width, it.height
                    ));
                }
            }
            if it.item_type == "jpeg" || ct == "image/jpeg" {
                let b = item_data(data, it);
                if b.starts_with(&[0xFF, 0xD8]) {
                    thumb = Some(b);
                }
            }
        }
    }
    // unreferenced bytes
    let mut ext: Vec<(u64, u64)> = all.iter().map(|&(s, e, _)| (s, e)).collect();
    ext.sort();
    let mut unref = 0u64;
    for &(hs, he) in &c.holes {
        let (hs, he) = (hs as u64, he as u64);
        let mut covered = 0u64;
        let mut cur = hs;
        for &(s, e) in &ext {
            let (s, e) = (s.max(cur), e.min(he));
            if s < e {
                covered += e - s;
                cur = e;
            }
        }
        unref += (he - hs) - covered.min(he - hs);
    }
    c.info.unreferenced_bytes = unref;

    let mut p = empty(Format::Heif);
    p.width = w;
    p.height = h;
    p.structure = structure;
    let mut problems = std::mem::take(&mut c.problems);
    problems.dedup();
    if !problems.is_empty() {
        p.signals
            .push(sig("HEIF_STRUCTURE_ANOMALY").with("problems", problems.join("; ")));
    }
    if unref > 0 {
        p.signals
            .push(sig("HEIF_UNREFERENCED_DATA").with("bytes", unref.to_string()));
    }
    p.metadata = c.md;
    p.heif = Some(c.info);
    p.thumbnail = thumb;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(ty: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(body);
        v
    }
    fn full(ty: &[u8], v: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![v];
        b.extend(&flags.to_be_bytes()[1..]);
        b.extend(body);
        bx(ty, &b)
    }
    fn infe(id: u16, ty: &[u8], hidden: bool) -> Vec<u8> {
        let mut b = id.to_be_bytes().to_vec();
        b.extend([0, 0]);
        b.extend(ty);
        b.push(0);
        full(b"infe", 2, hidden as u32, &b)
    }

    /// ftyp + meta(hdlr, pitm 1, iinf(1 hvc1), iloc v`ver`, iprp(ispe 64x48)) + mdat(`payload`).
    fn file(ver: u8, payload: &[u8], extent_len: u32) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let hdlr = full(b"hdlr", 0, 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let pitm = full(b"pitm", 0, 0, &1u16.to_be_bytes());
        let mut iinf_b = 1u16.to_be_bytes().to_vec();
        iinf_b.extend(infe(1, b"hvc1", false));
        let iinf = full(b"iinf", 0, 0, &iinf_b);
        let mut ispe_b = 64u32.to_be_bytes().to_vec();
        ispe_b.extend(48u32.to_be_bytes());
        let ipco = bx(b"ipco", &full(b"ispe", 0, 0, &ispe_b));
        let ipma = full(b"ipma", 0, 0, &[0, 0, 0, 1, 0, 1, 1, 0x81]);
        let iprp = bx(b"iprp", &[ipco, ipma].concat());
        let build = |off: u32| {
            let mut b = vec![0x44, 0x00];
            if ver < 2 {
                b.extend(1u16.to_be_bytes());
                b.extend(1u16.to_be_bytes());
            } else {
                b.extend(1u32.to_be_bytes());
                b.extend(1u32.to_be_bytes());
            }
            if ver >= 1 {
                b.extend(0u16.to_be_bytes());
            }
            b.extend(0u16.to_be_bytes()); // dri
            b.extend(1u16.to_be_bytes());
            b.extend(off.to_be_bytes());
            b.extend(extent_len.to_be_bytes());
            full(b"iloc", ver, 0, &b)
        };
        let pre = |iloc: &Vec<u8>| {
            let meta = full(
                b"meta",
                0,
                0,
                &[
                    hdlr.clone(),
                    pitm.clone(),
                    iinf.clone(),
                    iloc.clone(),
                    iprp.clone(),
                ]
                .concat(),
            );
            ftyp.len() + meta.len() + 8
        };
        let off = pre(&build(0)) as u32;
        let iloc = build(off);
        let meta = full(b"meta", 0, 0, &[hdlr, pitm, iinf, iloc, iprp].concat());
        [ftyp, meta, bx(b"mdat", payload)].concat()
    }

    #[test]
    fn iloc_versions_and_coverage() {
        for ver in 0..=2 {
            let f = file(ver, &[7u8; 100], 100);
            let p = parse(&f).unwrap();
            let h = p.heif.as_ref().unwrap();
            assert_eq!((p.width, p.height), (64, 48), "v{ver}");
            assert_eq!(h.items[0].extents.len(), 1);
            assert_eq!(h.unreferenced_bytes, 0);
            assert!(p.signals.is_empty(), "v{ver}: {:?}", p.signals);
            let total: u64 = p.structure.iter().map(|s| s.length).sum();
            assert_eq!(total as usize, f.len());
        }
        let p = parse(&file(1, &[7u8; 100], 60)).unwrap();
        assert_eq!(p.heif.as_ref().unwrap().unreferenced_bytes, 40);
        assert_eq!(p.signals[0].code, "HEIF_UNREFERENCED_DATA");
        // extent beyond file
        let p = parse(&file(1, &[7u8; 10], 60)).unwrap();
        assert_eq!(p.signals[0].code, "HEIF_STRUCTURE_ANOMALY");
    }

    #[test]
    fn truncated_and_hostile_boxes() {
        let f = file(1, &[7u8; 100], 100);
        for cut in [9usize, 20, 40, 80, f.len() - 50] {
            let _ = parse(&f[..cut]);
        }
        // box size smaller than its header, huge largesize
        let mut f = bx(b"ftyp", b"heic\0\0\0\0");
        f.extend([0, 0, 0, 3, b'm', b'e', b't', b'a']);
        let p = parse(&f).unwrap();
        assert_eq!(p.structure.last().unwrap().kind, "trailer");
        let mut f = bx(b"ftyp", b"heic\0\0\0\0");
        f.extend([0, 0, 0, 1, b'm', b'd', b'a', b't']);
        f.extend(u64::MAX.to_be_bytes());
        let p = parse(&f).unwrap();
        assert!(p.signals.iter().any(|s| s.code == "HEIF_STRUCTURE_ANOMALY"));
        assert!(parse(b"\0\0\0\x0cftypisom").is_err());
    }
}
