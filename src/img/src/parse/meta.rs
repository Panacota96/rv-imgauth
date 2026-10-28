//! Metadata readers shared by every format: EXIF (kamadak-exif), XMP (quick-xml, with xmpMM:History), IPTC-IIM
//! (from a Photoshop image resource block) and the ICC profile hash. Readers never fail: what cannot be read is
//! written to `Metadata::problems` and the rest is kept.

use super::{SegExt, be16, be32, cap, seg, text};
use crate::model::{HistoryEvent, Metadata, Segment, Tag};
use exif::{Context, Field, In, Value};
use quick_xml::events::Event;
use sha2::{Digest, Sha256};

/// Longest value text kept for one tag.
const MAX_VALUE: usize = 512;

pub fn sha256_hex(data: &[u8]) -> String {
    let h = Sha256::digest(data);
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads a TIFF-structured EXIF block (`origin` names where it came from, for problems). Returns the IFD1 JPEG
/// thumbnail when present. A leading "Exif\0\0" is tolerated.
pub fn read_exif(tiff: &[u8], md: &mut Metadata, origin: &str) -> Option<Vec<u8>> {
    let tiff = tiff.strip_prefix(b"Exif\0\0").unwrap_or(tiff);
    let exif = match exif::Reader::new()
        .continue_on_error(true)
        .read_raw(tiff.to_vec())
    {
        Ok(e) => e,
        Err(e) => {
            let mut errs = vec![];
            match e.distill_partial_result(|v| errs = v) {
                Ok(exif) => {
                    for e in errs.iter().take(10) {
                        md.problems.push(format!("EXIF ({origin}): {e}"));
                    }
                    exif
                }
                Err(e) => {
                    md.problems
                        .push(format!("EXIF ({origin}) could not be read: {e}"));
                    return None;
                }
            }
        }
    };
    for f in exif.fields() {
        let group = match (f.tag.context(), f.ifd_num) {
            (Context::Exif, _) => "Exif".to_string(),
            (Context::Gps, _) => "GPS".to_string(),
            (Context::Interop, _) => "Interop".to_string(),
            (_, In(0)) => "IFD0".to_string(),
            (_, In(1)) => "IFD1".to_string(),
            (_, In(n)) => format!("IFD{n}"),
        };
        if f.tag == exif::Tag::MakerNote {
            md.makernote_present = true;
        }
        let name = match f.tag.description() {
            Some(_) => f.tag.to_string(),
            None => format!("Tag0x{:04X}", f.tag.number()),
        };
        md.tags.push(Tag {
            group,
            id: f.tag.number() as u32,
            name,
            value: render(f),
        });
    }
    // IFD1 JPEG thumbnail
    let off = exif
        .get_field(exif::Tag::JPEGInterchangeFormat, In::THUMBNAIL)
        .and_then(|f| f.value.get_uint(0));
    let len = exif
        .get_field(exif::Tag::JPEGInterchangeFormatLength, In::THUMBNAIL)
        .and_then(|f| f.value.get_uint(0));
    match (off, len) {
        (Some(o), Some(l)) => {
            let (o, l) = (o as usize, l as usize);
            match tiff.get(o..o.saturating_add(l)) {
                Some(t) if t.starts_with(&[0xFF, 0xD8]) => Some(t.to_vec()),
                Some(_) => {
                    md.problems.push(format!(
                        "EXIF ({origin}): the IFD1 thumbnail at offset {o} is not a JPEG"
                    ));
                    None
                }
                None => {
                    md.problems.push(format!(
                        "EXIF ({origin}): the IFD1 thumbnail ({l} bytes at offset {o}) lies outside the EXIF block"
                    ));
                    None
                }
            }
        }
        (Some(_), None) | (None, Some(_)) => {
            md.problems.push(format!(
                "EXIF ({origin}): IFD1 has only one of JPEGInterchangeFormat/JPEGInterchangeFormatLength"
            ));
            None
        }
        _ => None,
    }
}

/// Value as display text. ASCII is raw text; integers whose display differs from the number keep both
/// ("6 (row 0 at right and column 0 at top)") so the signals can read the number back.
fn render(f: &Field) -> String {
    match &f.value {
        Value::Ascii(v) => {
            let parts: Vec<String> = v.iter().map(|s| text(s, MAX_VALUE)).collect();
            cap(&parts.join(", "), MAX_VALUE)
        }
        Value::Undefined(b, _) if f.tag == exif::Tag::MakerNote => format!("<{} bytes>", b.len()),
        Value::Undefined(b, _) if f.tag == exif::Tag::UserComment => user_comment(b),
        Value::Byte(_) | Value::Short(_) | Value::Long(_)
            if f.value.iter_uint().map(|i| i.len()) == Some(1) =>
        {
            let n = f.value.get_uint(0).unwrap_or(0);
            let disp = f.display_value().to_string();
            if disp == n.to_string() {
                disp
            } else {
                cap(&format!("{n} ({disp})"), MAX_VALUE)
            }
        }
        _ => cap(&f.display_value().to_string(), MAX_VALUE),
    }
}

fn user_comment(b: &[u8]) -> String {
    if b.len() < 8 {
        return text(b, MAX_VALUE);
    }
    let (code, body) = b.split_at(8);
    if code.starts_with(b"UNICODE") {
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        cap(
            s.trim_matches(|c: char| c == '\0' || c.is_whitespace()),
            MAX_VALUE,
        )
    } else {
        text(body, MAX_VALUE)
    }
}

// ---------------------------------------------------------------------------------------------------- XMP

/// Canonical prefixes for the namespaces the signals look at (files may bind other prefixes to them).
const NAMESPACES: &[(&str, &str)] = &[
    ("http://ns.adobe.com/xap/1.0/", "xmp"),
    ("http://ns.adobe.com/xap/1.0/mm/", "xmpMM"),
    ("http://ns.adobe.com/xap/1.0/sType/ResourceEvent#", "stEvt"),
    ("http://ns.adobe.com/xap/1.0/sType/ResourceRef#", "stRef"),
    ("http://ns.adobe.com/xap/1.0/rights/", "xmpRights"),
    ("http://ns.adobe.com/photoshop/1.0/", "photoshop"),
    ("http://ns.adobe.com/exif/1.0/", "exif"),
    ("http://ns.adobe.com/exif/1.0/aux/", "aux"),
    ("http://cipa.jp/exif/1.0/", "exifEX"),
    ("http://ns.adobe.com/tiff/1.0/", "tiff"),
    ("http://purl.org/dc/elements/1.1/", "dc"),
    ("http://ns.adobe.com/camera-raw-settings/1.0/", "crs"),
    ("http://ns.adobe.com/lightroom/1.0/", "lr"),
    ("http://www.gimp.org/xmp/", "GIMP"),
    (
        "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/",
        "Iptc4xmpCore",
    ),
    ("http://www.w3.org/1999/02/22-rdf-syntax-ns#", "rdf"),
    ("adobe:ns:meta/", "x"),
];

struct Item {
    text: String,
    fields: Vec<(String, String)>,
}

struct Prop {
    name: String,
    depth: usize,
    text: String,
    items: Vec<Item>,
}

/// Parses an XMP packet: simple properties (attributes or element text, arrays joined with "; ") become tags of
/// group "XMP" named "prefix:Local"; xmpMM:History entries become [`HistoryEvent`]s.
pub fn read_xmp(packet: &str, md: &mut Metadata) {
    if md.xmp.is_some() {
        md.problems
            .push("more than one XMP packet; the later one was also read".into());
        let prev = md.xmp.take().unwrap_or_default();
        md.xmp = Some(format!("{prev}\n{packet}"));
    } else {
        md.xmp = Some(packet.to_string());
    }
    if let Err(e) = parse_xmp(packet, md) {
        md.problems.push(format!("XMP: {e}"));
    }
}

fn parse_xmp(packet: &str, md: &mut Metadata) -> Result<(), String> {
    let mut reader = quick_xml::Reader::from_str(packet);
    // scopes: namespace bindings per open element
    let mut scopes: Vec<Vec<(String, String)>> = vec![];
    let mut stack: Vec<String> = vec![];
    let mut top_desc: Option<usize> = None; // depth of the top-level rdf:Description
    let mut prop: Option<Prop> = None;
    let mut item: Option<(usize, Item)> = None; // depth of rdf:li
    let mut field: Option<(usize, String, String)> = None; // depth, name, text
    let mut events = 0usize;
    loop {
        events += 1;
        if events > 1_000_000 {
            return Err("packet too complex".into());
        }
        let ev = reader
            .read_event()
            .map_err(|e| format!("malformed XML at byte {}: {e}", reader.buffer_position()))?;
        let (start, empty) = match &ev {
            Event::Start(s) => (Some(s.clone()), false),
            Event::Empty(s) => (Some(s.clone()), true),
            _ => (None, false),
        };
        if let Some(s) = start {
            let raw_name = s.name().into_inner().to_string();
            // namespace declarations first
            let mut decls = vec![];
            let mut attrs = vec![];
            for a in s.attributes().with_checks(false).flatten() {
                let k = a.key.into_inner().to_string();
                let v = match quick_xml::escape::unescape(&a.value) {
                    Ok(v) => v.into_owned(),
                    Err(_) => a.value.to_string(),
                };
                if k == "xmlns" {
                    decls.push((String::new(), v));
                } else if let Some(p) = k.strip_prefix("xmlns:") {
                    decls.push((p.to_string(), v));
                } else {
                    attrs.push((k, v));
                }
            }
            scopes.push(decls);
            let name = canonical(&raw_name, &scopes);
            let attrs: Vec<(String, String)> = attrs
                .into_iter()
                .map(|(k, v)| (canonical(&k, &scopes), v))
                .collect();
            let depth = stack.len();
            let parent = stack.last().cloned().unwrap_or_default();
            stack.push(name.clone());

            if name == "rdf:Description" && parent == "rdf:RDF" {
                top_desc = Some(depth);
                for (k, v) in &attrs {
                    if is_property(k) {
                        md.tags.push(xmp_tag(k, v));
                    }
                }
            } else if top_desc.is_some_and(|d| depth == d + 1) {
                prop = Some(Prop {
                    name: name.clone(),
                    depth,
                    text: String::new(),
                    items: vec![],
                });
                // property with rdf:parseType="Resource" or attribute qualifiers: keep the simple fields as an item
                let fields: Vec<(String, String)> = attrs
                    .iter()
                    .filter(|(k, _)| is_property(k))
                    .cloned()
                    .collect();
                if !fields.is_empty()
                    && let Some(p) = prop.as_mut()
                {
                    p.items.push(Item {
                        text: String::new(),
                        fields,
                    });
                }
            } else if prop.is_some() && name == "rdf:li" && item.is_none() {
                let fields = attrs
                    .iter()
                    .filter(|(k, _)| is_property(k))
                    .cloned()
                    .collect();
                item = Some((
                    depth,
                    Item {
                        text: String::new(),
                        fields,
                    },
                ));
            } else if let Some((_, it)) = item.as_mut() {
                if name == "rdf:Description" {
                    it.fields
                        .extend(attrs.iter().filter(|(k, _)| is_property(k)).cloned());
                } else if !name.starts_with("rdf:") && field.is_none() {
                    field = Some((depth, name.clone(), String::new()));
                }
            } else if let Some(p) = prop.as_mut()
                && name != "rdf:Description"
                && !name.starts_with("rdf:")
                && field.is_none()
            {
                // struct property: <xmpMM:DerivedFrom rdf:parseType="Resource"><stRef:x>…
                field = Some((depth, name.clone(), String::new()));
                let _ = p;
            }
            if empty {
                close(
                    &mut stack,
                    &mut scopes,
                    &mut top_desc,
                    &mut prop,
                    &mut item,
                    &mut field,
                    md,
                );
            }
            continue;
        }
        match ev {
            Event::Text(t) => push_text(&t.xml10_content(), &mut prop, &mut item, &mut field),
            Event::CData(t) => push_text(&t.xml10_content(), &mut prop, &mut item, &mut field),
            Event::GeneralRef(r) => {
                let s = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => match &*r.xml10_content() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => format!("&{other};"),
                    },
                };
                push_text(&s, &mut prop, &mut item, &mut field);
            }
            Event::End(_) => close(
                &mut stack,
                &mut scopes,
                &mut top_desc,
                &mut prop,
                &mut item,
                &mut field,
                md,
            ),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(format!(
            "packet ends inside <{}>",
            stack.last().cloned().unwrap_or_default()
        ));
    }
    Ok(())
}

fn push_text(
    s: &str,
    prop: &mut Option<Prop>,
    item: &mut Option<(usize, Item)>,
    field: &mut Option<(usize, String, String)>,
) {
    if let Some((_, _, t)) = field.as_mut() {
        t.push_str(s);
    } else if let Some((_, it)) = item.as_mut() {
        it.text.push_str(s);
    } else if let Some(p) = prop.as_mut() {
        p.text.push_str(s);
    }
}

#[allow(clippy::too_many_arguments)]
fn close(
    stack: &mut Vec<String>,
    scopes: &mut Vec<Vec<(String, String)>>,
    top_desc: &mut Option<usize>,
    prop: &mut Option<Prop>,
    item: &mut Option<(usize, Item)>,
    field: &mut Option<(usize, String, String)>,
    md: &mut Metadata,
) {
    let depth = stack.len().saturating_sub(1);
    stack.pop();
    scopes.pop();
    if field.as_ref().is_some_and(|(d, _, _)| *d == depth) {
        let (_, n, t) = field.take().unwrap_or_default();
        let t = t.trim().to_string();
        if let Some((_, it)) = item.as_mut() {
            it.fields.push((n, t));
        } else if let Some(p) = prop.as_mut() {
            if p.items.is_empty() {
                p.items.push(Item {
                    text: String::new(),
                    fields: vec![],
                });
            }
            if let Some(it) = p.items.last_mut() {
                it.fields.push((n, t));
            }
        }
    } else if item.as_ref().is_some_and(|(d, _)| *d == depth) {
        if let Some((_, it)) = item.take()
            && let Some(p) = prop.as_mut()
        {
            p.items.push(it);
        }
    } else if prop.as_ref().is_some_and(|p| p.depth == depth) {
        if let Some(p) = prop.take() {
            finish_prop(p, md);
        }
    } else if top_desc.is_some_and(|d| d == depth) {
        *top_desc = None;
    }
}

fn finish_prop(p: Prop, md: &mut Metadata) {
    if p.name == "xmpMM:History" {
        for it in &p.items {
            let get = |k: &str| {
                it.fields
                    .iter()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            md.xmp_history.push(HistoryEvent {
                action: get("stEvt:action"),
                when: get("stEvt:when"),
                software_agent: get("stEvt:softwareAgent"),
                changed: get("stEvt:changed"),
                instance_id: get("stEvt:instanceID"),
            });
        }
        md.tags.push(xmp_tag(
            "xmpMM:History",
            &format!("{} event(s)", p.items.len()),
        ));
        return;
    }
    let text = p.text.trim();
    let values: Vec<&str> = p
        .items
        .iter()
        .map(|i| i.text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !values.is_empty() {
        md.tags.push(xmp_tag(&p.name, &values.join("; ")));
    } else if !text.is_empty() {
        md.tags.push(xmp_tag(&p.name, text));
    } else {
        // a structure: keep its simple fields as "prop/field"
        for it in &p.items {
            for (k, v) in &it.fields {
                md.tags.push(xmp_tag(&format!("{}/{k}", p.name), v));
            }
        }
    }
}

fn xmp_tag(name: &str, value: &str) -> Tag {
    Tag {
        group: "XMP".into(),
        id: 0,
        name: name.to_string(),
        value: cap(value.trim(), MAX_VALUE),
    }
}

fn is_property(k: &str) -> bool {
    !(k.starts_with("rdf:") || k.starts_with("xml:") || k.starts_with("xmlns") || k == "x:xmptk")
}

/// "p:Local" with p replaced by the canonical prefix of its namespace URI (unknown URIs keep the file's prefix).
fn canonical(qname: &str, scopes: &[Vec<(String, String)>]) -> String {
    let (prefix, local) = match qname.split_once(':') {
        Some((p, l)) => (p, l),
        None => ("", qname),
    };
    if prefix == "xml" {
        return qname.to_string();
    }
    let uri = scopes
        .iter()
        .rev()
        .flat_map(|s| s.iter())
        .find(|(p, _)| p == prefix)
        .map(|(_, u)| u.as_str());
    let canon = uri
        .and_then(|u| NAMESPACES.iter().find(|(n, _)| *n == u))
        .map(|(_, p)| *p)
        .unwrap_or(prefix);
    if canon.is_empty() {
        local.to_string()
    } else {
        format!("{canon}:{local}")
    }
}

// ---------------------------------------------------------------------------------------------- Photoshop/IPTC

/// Walks a Photoshop "Photoshop 3.0\0" image resource block (8BIM resources). `base` is the file offset of
/// `irb[0]`. Returns one child segment per resource; IPTC-IIM (0x0404) is read into `md`.
pub fn read_photoshop_irb(irb: &[u8], base: usize, md: &mut Metadata) -> Vec<Segment> {
    let mut out = vec![];
    let mut p = if irb.starts_with(b"Photoshop 3.0\0") {
        14
    } else {
        0
    };
    while p + 12 <= irb.len() {
        let sig = &irb[p..p + 4];
        if sig != b"8BIM" && sig != b"MeSa" && sig != b"PHUT" && sig != b"AgHg" && sig != b"DCSR" {
            md.problems.push(format!(
                "Photoshop resource block: unexpected signature at byte {} of the block",
                p
            ));
            break;
        }
        let id = be16(irb, p + 4).unwrap_or(0);
        let nlen = irb[p + 6] as usize;
        let mut q = p + 7 + nlen;
        if q % 2 == 1 {
            q += 1; // pascal string padded to even length (including the length byte)
        }
        let Some(size) = be32(irb, q) else {
            md.problems
                .push("Photoshop resource block truncated".to_string());
            break;
        };
        let size = size as usize;
        let data_start = q + 4;
        let Some(data) = irb.get(data_start..data_start.saturating_add(size)) else {
            md.problems.push(format!(
                "Photoshop resource 0x{id:04X} declares {size} bytes beyond the block"
            ));
            out.push(
                seg("8BIM", base + p, irb.len() - p)
                    .d("id", format!("0x{id:04X}"))
                    .d("truncated", "true"),
            );
            break;
        };
        let mut end = data_start + size;
        if end % 2 == 1 && end < irb.len() {
            end += 1;
        }
        out.push(seg("8BIM", base + p, end - p).d("id", format!("0x{id:04X}")));
        if id == 0x0404 {
            read_iptc(data, md);
        }
        p = end;
    }
    out
}

fn iptc_name(record: u8, ds: u8) -> Option<&'static str> {
    Some(match (record, ds) {
        (1, 0) => "EnvelopeRecordVersion",
        (1, 90) => "CodedCharacterSet",
        (2, 0) => "ApplicationRecordVersion",
        (2, 5) => "ObjectName",
        (2, 10) => "Urgency",
        (2, 15) => "Category",
        (2, 20) => "SupplementalCategories",
        (2, 25) => "Keywords",
        (2, 40) => "SpecialInstructions",
        (2, 55) => "DateCreated",
        (2, 60) => "TimeCreated",
        (2, 62) => "DigitalCreationDate",
        (2, 63) => "DigitalCreationTime",
        (2, 65) => "OriginatingProgram",
        (2, 70) => "ProgramVersion",
        (2, 80) => "By-line",
        (2, 85) => "By-lineTitle",
        (2, 90) => "City",
        (2, 92) => "Sub-location",
        (2, 95) => "Province-State",
        (2, 100) => "Country-PrimaryLocationCode",
        (2, 101) => "Country-PrimaryLocationName",
        (2, 103) => "OriginalTransmissionReference",
        (2, 105) => "Headline",
        (2, 110) => "Credit",
        (2, 115) => "Source",
        (2, 116) => "CopyrightNotice",
        (2, 120) => "Caption-Abstract",
        (2, 122) => "Writer-Editor",
        _ => return None,
    })
}

/// Reads IPTC-IIM datasets (0x1C record dataset size data).
pub fn read_iptc(d: &[u8], md: &mut Metadata) {
    let mut p = 0usize;
    while p < d.len() {
        if d[p] != 0x1C {
            if d[p..].iter().any(|&b| b != 0) {
                md.problems
                    .push(format!("IPTC: unexpected byte 0x{:02X} at {p}", d[p]));
            }
            return;
        }
        let (Some(&rec), Some(&ds), Some(len)) = (d.get(p + 1), d.get(p + 2), be16(d, p + 3))
        else {
            md.problems.push("IPTC: dataset header truncated".into());
            return;
        };
        let mut q = p + 5;
        let len = if len & 0x8000 != 0 {
            // extended dataset: the low 15 bits give the length of the length field
            let n = (len & 0x7FFF) as usize;
            if n == 0 || n > 4 || q + n > d.len() {
                md.problems
                    .push("IPTC: invalid extended dataset length".into());
                return;
            }
            let v = d[q..q + n]
                .iter()
                .fold(0usize, |a, &b| (a << 8) | b as usize);
            q += n;
            v
        } else {
            len as usize
        };
        let Some(val) = d.get(q..q.saturating_add(len)) else {
            md.problems
                .push(format!("IPTC: dataset {rec}:{ds} truncated"));
            return;
        };
        let name = iptc_name(rec, ds)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{rec}:{ds:03}"));
        let value = if rec == 1 && ds == 90 {
            val.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        } else if rec == 2 && ds == 0 && val.len() == 2 {
            u16::from_be_bytes([val[0], val[1]]).to_string()
        } else {
            text(val, MAX_VALUE)
        };
        md.tags.push(Tag {
            group: "IPTC".into(),
            id: rec as u32 * 1000 + ds as u32,
            name,
            value,
        });
        p = q + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff_le(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        // minimal little-endian TIFF with one IFD; values > 4 bytes placed after the IFD
        let mut out = b"II*\0\x08\0\0\0".to_vec();
        let n = entries.len();
        let mut extra: Vec<u8> = vec![];
        let extra_base = 8 + 2 + n * 12 + 4;
        out.extend((n as u16).to_le_bytes());
        for (tag, typ, count, val) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(count.to_le_bytes());
            if val.len() <= 4 {
                let mut v = val.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend(((extra_base + extra.len()) as u32).to_le_bytes());
                extra.extend(val);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(extra);
        out
    }

    #[test]
    fn exif_basic_tags() {
        let t = tiff_le(&[
            (0x010F, 2, 6, b"Canon\0".to_vec()),
            (0x0112, 3, 1, 6u16.to_le_bytes().to_vec()),
            (0x0131, 2, 10, b"GIMP 2.10\0".to_vec()),
        ]);
        let mut md = Metadata::default();
        assert!(read_exif(&t, &mut md, "test").is_none());
        assert_eq!(md.get("IFD0", "Make"), Some("Canon"));
        assert_eq!(md.get("IFD0", "Software"), Some("GIMP 2.10"));
        assert!(md.get("IFD0", "Orientation").unwrap().starts_with("6 ("));
        assert!(md.problems.is_empty(), "{:?}", md.problems);
    }

    #[test]
    fn exif_garbage_is_a_problem_not_a_panic() {
        let mut md = Metadata::default();
        read_exif(b"II*\0\xff\xff\xff\x7f", &mut md, "t");
        read_exif(b"", &mut md, "t");
        read_exif(b"MM\0*\0\0\0\x08\xff\xff", &mut md, "t");
        assert!(!md.problems.is_empty());
    }

    #[test]
    fn exif_thumbnail_out_of_range() {
        let t = tiff_le(&[(0x010F, 2, 6, b"Canon\0".to_vec())]);
        // patch "next IFD" to an IFD1 holding thumbnail tags pointing out of range
        let mut t = t;
        let ifd1 = t.len() as u32;
        let next_pos = 8 + 2 + 12;
        t[next_pos..next_pos + 4].copy_from_slice(&ifd1.to_le_bytes());
        t.extend(2u16.to_le_bytes());
        for (tag, v) in [(0x0201u16, 9999u32), (0x0202, 100)] {
            t.extend(tag.to_le_bytes());
            t.extend(4u16.to_le_bytes());
            t.extend(1u32.to_le_bytes());
            t.extend(v.to_le_bytes());
        }
        t.extend(0u32.to_le_bytes());
        let mut md = Metadata::default();
        assert!(read_exif(&t, &mut md, "t").is_none());
        assert!(
            md.problems.iter().any(|p| p.contains("outside")),
            "{:?}",
            md.problems
        );
    }

    const XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:mm="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:stEvt="http://ns.adobe.com/xap/1.0/sType/ResourceEvent#"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmp:CreatorTool="GIMP 2.10"
    xmp:ModifyDate="2024-03-12T10:00:00+01:00">
   <xmp:MetadataDate>2024-03-12T10:00:00&#43;01:00</xmp:MetadataDate>
   <dc:creator><rdf:Seq><rdf:li>A &amp; B</rdf:li></rdf:Seq></dc:creator>
   <exif:UserComment><rdf:Alt><rdf:li xml:lang="x-default">Screenshot</rdf:li></rdf:Alt></exif:UserComment>
   <mm:History>
    <rdf:Seq>
     <rdf:li stEvt:action="saved" stEvt:softwareAgent="Gimp 2.10 (Linux)" stEvt:when="2024-03-11T09:00:00+01:00" stEvt:changed="/"/>
     <rdf:li rdf:parseType="Resource">
      <stEvt:action>saved</stEvt:action>
      <stEvt:softwareAgent>Adobe Photoshop 25.0</stEvt:softwareAgent>
      <stEvt:instanceID>xmp.iid:2</stEvt:instanceID>
     </rdf:li>
    </rdf:Seq>
   </mm:History>
   <mm:DerivedFrom rdf:parseType="Resource"><stRef:documentID xmlns:stRef="http://ns.adobe.com/xap/1.0/sType/ResourceRef#">doc1</stRef:documentID></mm:DerivedFrom>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn xmp_properties_and_history() {
        let mut md = Metadata::default();
        read_xmp(XMP, &mut md);
        assert!(md.problems.is_empty(), "{:?}", md.problems);
        assert_eq!(md.get("XMP", "xmp:CreatorTool"), Some("GIMP 2.10"));
        assert_eq!(
            md.get("XMP", "xmp:ModifyDate"),
            Some("2024-03-12T10:00:00+01:00")
        );
        assert_eq!(
            md.get("XMP", "xmp:MetadataDate"),
            Some("2024-03-12T10:00:00+01:00")
        );
        assert_eq!(md.get("XMP", "dc:creator"), Some("A & B"));
        assert_eq!(md.get("XMP", "exif:UserComment"), Some("Screenshot"));
        assert_eq!(
            md.get("XMP", "xmpMM:DerivedFrom/stRef:documentID"),
            Some("doc1")
        );
        assert_eq!(md.xmp_history.len(), 2);
        assert_eq!(md.xmp_history[0].software_agent, "Gimp 2.10 (Linux)");
        assert_eq!(md.xmp_history[0].changed, "/");
        assert_eq!(md.xmp_history[1].software_agent, "Adobe Photoshop 25.0");
        assert_eq!(md.xmp_history[1].instance_id, "xmp.iid:2");
    }

    #[test]
    fn xmp_malformed_is_a_problem() {
        let mut md = Metadata::default();
        read_xmp("<x:xmpmeta><rdf:RDF></x:xmpmeta>", &mut md);
        assert!(!md.problems.is_empty());
        let mut md = Metadata::default();
        read_xmp("<a><b>", &mut md);
        assert!(!md.problems.is_empty());
    }

    #[test]
    fn iptc_from_irb() {
        let mut iim = vec![0x1C, 2, 0, 0, 2, 0, 4];
        iim.extend([0x1C, 2, 40, 0, 8]);
        iim.extend(b"FBMD0123");
        iim.extend([0x1C, 2, 5, 0x80, 2, 0, 3]);
        iim.extend(b"abc");
        let mut irb = b"Photoshop 3.0\0".to_vec();
        irb.extend(b"8BIM");
        irb.extend(0x0404u16.to_be_bytes());
        irb.extend([0, 0]); // empty name, padded
        irb.extend((iim.len() as u32).to_be_bytes());
        irb.extend(&iim);
        if iim.len() % 2 == 1 {
            irb.push(0);
        }
        let mut md = Metadata::default();
        let kids = read_photoshop_irb(&irb, 100, &mut md);
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].offset, 114);
        assert_eq!(md.get("IPTC", "SpecialInstructions"), Some("FBMD0123"));
        assert_eq!(md.get("IPTC", "ObjectName"), Some("abc"));
        assert_eq!(md.get("IPTC", "ApplicationRecordVersion"), Some("4"));
        assert!(md.problems.is_empty(), "{:?}", md.problems);
        // truncated
        let mut md = Metadata::default();
        read_photoshop_irb(&irb[..irb.len() - 5], 0, &mut md);
        assert!(!md.problems.is_empty());
    }

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
