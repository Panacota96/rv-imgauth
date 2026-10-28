//! Structure and metadata signals. Each signal names an observation and its parameters for the i18n text; none
//! is a verdict. Codes, severities and parameter names follow the finding catalogue.
//!
//! Structure signals are computed by each parser at parse time (they need raw bytes) and stored in
//! `ParsedImage::signals`; [`structure`] returns them. Metadata signals are computed here from
//! `ParsedImage::metadata`.

use crate::model::{Format, ParsedImage, Signal};

/// Catalogue severities of the codes this module emits.
pub fn severity(code: &str) -> &'static str {
    match code {
        "PROGRESSIVE_ENCODING"
        | "PNG_PRIVATE_CHUNKS"
        | "METADATA_STRIPPED"
        | "MESSENGER_TRACES"
        | "MAKERNOTE_MISSING_FOR_MAKE"
        | "METADATA_PROBLEMS" => "low",
        _ => "medium",
    }
}

pub(crate) fn sig(code: &str) -> Signal {
    Signal::new(code, severity(code))
}

pub fn structure(p: &ParsedImage) -> Vec<Signal> {
    p.signals.clone()
}

// ------------------------------------------------------------------------------------------------- JPEG

fn is_table_or_frame(m: &str) -> bool {
    m == "DQT" || m == "DHT" || m == "DRI" || m == "DAC" || m.starts_with("SOF")
}

/// Compact marker list: consecutive repeats as "DQT×2", scans elided after the first SOS.
pub(crate) fn compact_markers(markers: &[String]) -> String {
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    let mut after_sos = false;
    while i < markers.len() {
        let m = &markers[i];
        let mut n = 1;
        while i + n < markers.len() && &markers[i + n] == m {
            n += 1;
        }
        if after_sos && m != "EOI" {
            if out.last().map(String::as_str) != Some("…") {
                out.push("…".into());
            }
        } else if n > 1 {
            out.push(format!("{m}×{n}"));
        } else {
            out.push(m.clone());
        }
        if m == "SOS" {
            after_sos = true;
        }
        i += n;
    }
    out.join(", ")
}

pub(crate) fn jpeg_structure(p: &ParsedImage, mpf_bytes: u64) -> Vec<Signal> {
    let mut v = vec![];
    let Some(j) = &p.jpeg else { return v };
    let mk = &j.markers;
    // marker order
    let mut why: Vec<&str> = vec![];
    if let Some(i) = mk.iter().position(|m| m == "APP0/JFIF")
        && i != 1
    {
        why.push("the JFIF APP0 segment should come first, right after SOI");
    }
    if let Some(i) = mk.iter().position(|m| m == "APP1/Exif")
        && !(i == 1 || (i == 2 && mk[1].starts_with("APP0")))
    {
        why.push("the Exif APP1 segment should come right after SOI (or after the JFIF APP0)");
    }
    let camera =
        p.metadata.get("IFD0", "Make").is_some() || p.metadata.get("IFD0", "Model").is_some();
    if camera {
        let first_table = mk.iter().position(|m| is_table_or_frame(m));
        let sos = mk.iter().position(|m| m == "SOS").unwrap_or(mk.len());
        let last_app = mk[..sos].iter().rposition(|m| m.starts_with("APP"));
        if let (Some(t), Some(a)) = (first_table, last_app)
            && t < a
        {
            why.push("tables or the frame header come before application segments, which camera firmware does not normally write");
        }
    }
    if !why.is_empty() {
        v.push(
            sig("JPEG_MARKER_ORDER_UNUSUAL")
                .with("sequence", compact_markers(mk))
                .with("expected", why.join("; ")),
        );
    }
    // duplicate tables: DQT ids anywhere; DHT ids within one inter-scan interval
    let mut seen = std::collections::BTreeMap::<u8, usize>::new();
    for t in &j.qtables {
        *seen.entry(t.id).or_default() += 1;
    }
    let dup_q: Vec<String> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(id, _)| id.to_string())
        .collect();
    if !dup_q.is_empty() {
        v.push(
            sig("DUPLICATE_TABLES")
                .with("kind", "Quantization")
                .with("ids", dup_q.join(", ")),
        );
    }
    let mut dup_h: Vec<String> = vec![];
    let mut interval: Vec<String> = vec![];
    for s in &p.structure {
        if s.kind == "SOS" {
            interval.clear();
        } else if s.kind == "DHT"
            && let Some(t) = s.detail.get("tables")
        {
            for id in t.split(',').filter(|x| !x.is_empty()) {
                if interval.iter().any(|x| x == id) {
                    if !dup_h.iter().any(|x| x == id) {
                        dup_h.push(id.to_string());
                    }
                } else {
                    interval.push(id.to_string());
                }
            }
        }
    }
    if !dup_h.is_empty() {
        v.push(
            sig("DUPLICATE_TABLES")
                .with("kind", "Huffman")
                .with("ids", dup_h.join(", ")),
        );
    }
    if j.after_eoi > 0 {
        let mut s = sig("DATA_AFTER_END_MARKER")
            .with("bytes", j.after_eoi.to_string())
            .with("format", "JPEG");
        if mpf_bytes > 0 {
            s = s
                .with("mpf_bytes", mpf_bytes.to_string())
                .with("other_bytes", (j.after_eoi - mpf_bytes.min(j.after_eoi)).to_string())
                .with(
                    "note",
                    format!("{mpf_bytes} of these bytes are secondary images listed in the APP2/MPF index"),
                );
        }
        v.push(s);
    }
    if matches!(j.sof.as_str(), "SOF2" | "SOF6" | "SOF10" | "SOF14") {
        v.push(sig("PROGRESSIVE_ENCODING").with("sof", j.sof.clone()));
    }
    v
}

// -------------------------------------------------------------------------------------------------- PNG

/// Chunk types defined by the PNG 3rd edition and the registered extensions (incl. APNG).
pub(crate) const PNG_KNOWN: &[&str] = &[
    "IHDR", "PLTE", "IDAT", "IEND", "tRNS", "cHRM", "gAMA", "iCCP", "sBIT", "sRGB", "cICP", "mDCV",
    "cLLI", "tEXt", "zTXt", "iTXt", "bKGD", "hIST", "pHYs", "sPLT", "eXIf", "tIME", "acTL", "fcTL",
    "fdAT", "oFFs", "pCAL", "sCAL", "gIFg", "gIFx", "gIFt", "sTER", "fRAc", "dSIG",
];

/// Ordering rules of PNG 3rd edition §5.6 (W3C REC 2025-06-24), as problem texts.
pub(crate) fn png_order_problems(chunks: &[String]) -> Vec<String> {
    let mut out = vec![];
    let pos = |t: &str| chunks.iter().position(|c| c == t);
    let count = |t: &str| chunks.iter().filter(|c| *c == t).count();
    if chunks.first().map(String::as_str) != Some("IHDR") {
        out.push("IHDR is not the first chunk".to_string());
    }
    if pos("IEND").is_some_and(|i| i + 1 != chunks.len()) {
        out.push("IEND is not the last chunk".into());
    }
    let first_idat = pos("IDAT");
    let last_idat = chunks.iter().rposition(|c| c == "IDAT");
    match (first_idat, last_idat) {
        (None, _) | (_, None) => out.push("there is no IDAT chunk".into()),
        (Some(a), Some(b)) => {
            let between: Vec<&String> = chunks[a..=b].iter().filter(|c| *c != "IDAT").collect();
            if !between.is_empty() {
                let mut names: Vec<String> = between.iter().map(|s| s.to_string()).collect();
                names.dedup();
                out.push(format!(
                    "the IDAT chunks are not consecutive ({} between them)",
                    names.join(", ")
                ));
            }
        }
    }
    let plte = pos("PLTE");
    if let (Some(p), Some(i)) = (plte, first_idat)
        && p > i
    {
        out.push("PLTE comes after IDAT".into());
    }
    for t in [
        "cHRM", "gAMA", "iCCP", "sRGB", "cICP", "mDCV", "cLLI", "sBIT",
    ] {
        if let Some(i) = pos(t) {
            if plte.is_some_and(|p| i > p) {
                out.push(format!("{t} comes after PLTE"));
            } else if first_idat.is_some_and(|d| i > d) {
                out.push(format!("{t} comes after IDAT"));
            }
        }
    }
    for t in [
        "pHYs", "eXIf", "sPLT", "acTL", "oFFs", "pCAL", "sCAL", "sTER",
    ] {
        if let (Some(i), Some(d)) = (pos(t), first_idat)
            && i > d
        {
            out.push(format!("{t} comes after IDAT"));
        }
    }
    for t in ["tRNS", "bKGD", "hIST"] {
        if let Some(i) = pos(t) {
            if first_idat.is_some_and(|d| i > d) {
                out.push(format!("{t} comes after IDAT"));
            }
            if plte.is_some_and(|p| i < p) {
                out.push(format!("{t} comes before PLTE"));
            }
        }
    }
    if pos("iCCP").is_some() && pos("sRGB").is_some() {
        out.push("both iCCP and sRGB are present".into());
    }
    for t in [
        "IHDR", "PLTE", "IEND", "cHRM", "gAMA", "iCCP", "sRGB", "cICP", "mDCV", "cLLI", "sBIT",
        "pHYs", "eXIf", "tIME", "tRNS", "bKGD", "hIST", "acTL", "oFFs", "pCAL", "sCAL", "sTER",
    ] {
        let n = count(t);
        if n > 1 {
            out.push(format!("{t} appears {n} times"));
        }
    }
    out
}

pub(crate) fn png_structure(p: &ParsedImage, problems: &[String]) -> Vec<Signal> {
    let mut v = vec![];
    let Some(info) = &p.png else { return v };
    if !problems.is_empty() {
        v.push(sig("PNG_CHUNK_ORDER_INVALID").with("problems", problems.join("; ")));
    }
    let mut private: Vec<String> = info
        .chunks
        .iter()
        .filter(|c| !PNG_KNOWN.contains(&c.as_str()))
        .cloned()
        .collect();
    private.sort();
    private.dedup();
    if !private.is_empty() {
        v.push(sig("PNG_PRIVATE_CHUNKS").with("chunks", private.join(", ")));
    }
    if info.after_iend > 0 {
        v.push(
            sig("DATA_AFTER_END_MARKER")
                .with("bytes", info.after_iend.to_string())
                .with("format", "PNG"),
        );
    }
    v
}

// --------------------------------------------------------------------------------------------- metadata

/// Editors and processors named in Software / CreatorTool / history agents: (lower-case needle, name).
/// Matching is case-insensitive substring, except entries starting with '=' which must equal the whole value.
/// Camera firmware strings and phone OS versions are deliberately absent.
pub const EDITORS: &[(&str, &str)] = &[
    ("photoshop", "Adobe Photoshop"),
    ("lightroom", "Adobe Lightroom"),
    ("camera raw", "Adobe Camera Raw"),
    ("gimp", "GIMP"),
    ("paint.net", "Paint.NET"),
    ("affinity", "Affinity Photo"),
    ("pixelmator", "Pixelmator"),
    ("snapseed", "Snapseed"),
    ("picsart", "PicsArt"),
    ("canva", "Canva"),
    ("photopea", "Photopea"),
    ("acdsee", "ACDSee"),
    ("paintshop", "PaintShop Pro"),
    ("corel", "Corel"),
    ("luminar", "Luminar"),
    ("picasa", "Picasa"),
    ("windows photo", "Windows Photos"),
    ("microsoft photo", "Windows Photos"),
    ("microsoft paint", "Microsoft Paint"),
    ("mspaint", "Microsoft Paint"),
    ("=paint", "Microsoft Paint"),
    ("imagemagick", "ImageMagick"),
    ("graphicsmagick", "GraphicsMagick"),
    ("libvips", "libvips"),
    ("darktable", "darktable"),
    ("rawtherapee", "RawTherapee"),
    ("capture one", "Capture One"),
    ("dxo", "DxO"),
    ("krita", "Krita"),
    ("fotor", "Fotor"),
    ("facetune", "Facetune"),
    ("meitu", "Meitu"),
    ("vsco", "VSCO"),
    ("photoscape", "PhotoScape"),
    ("irfanview", "IrfanView"),
    ("xnview", "XnView"),
    ("faststone", "FastStone"),
];

pub fn editor_of(value: &str) -> Option<&'static str> {
    let l = value.trim().to_lowercase();
    EDITORS.iter().find_map(|(needle, name)| {
        let hit = match needle.strip_prefix('=') {
            Some(exact) => l == exact,
            None if *needle == "canva" => l
                .match_indices("canva")
                .any(|(i, _)| !l[i + 5..].starts_with('s')),
            None => l.contains(needle),
        };
        hit.then_some(*name)
    })
}

/// Seconds since 0000-03-01 style day count (proleptic Gregorian) for date comparisons.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe
}

/// Parses "YYYY:MM:DD HH:MM:SS", "YYYY-MM-DD HH:MM:SS" or ISO 8601 "YYYY-MM-DDTHH:MM[:SS][.f][Z|±hh:mm]".
/// Returns (local seconds, UTC offset in seconds if stated).
pub fn parse_date(s: &str) -> Option<(i64, Option<i64>)> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || y == 0 {
        return None;
    }
    let (mut h, mut mi, mut se) = (0, 0, 0);
    let mut rest = "";
    if b.len() >= 16 && (b[10] == b' ' || b[10] == b'T') {
        h = num(11..13)?;
        mi = num(14..16)?;
        rest = &s[16..];
        if rest.starts_with(':') && rest.len() >= 3 {
            se = num(17..19)?;
            rest = &s[19..];
        }
        if let Some(r) = rest.strip_prefix('.') {
            let k = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            rest = &r[k..];
        }
    }
    if h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let local = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se;
    Some((local, parse_offset(rest)))
}

/// "Z", "+01:00", "-0530" → seconds.
pub fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    if s == "Z" {
        return Some(0);
    }
    let sign = match s.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits: String = s[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 {
        return None;
    }
    let h: i64 = digits[..2].parse().ok()?;
    let m: i64 = digits[2..].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

fn hours(secs: i64) -> String {
    let h = secs as f64 / 3600.0;
    if h.abs() >= 48.0 {
        format!("{:.1} days", h / 24.0)
    } else {
        format!("{h:.1} h")
    }
}

fn leading_u32(s: &str) -> Option<u32> {
    let d: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    d.parse().ok()
}

const MAKERNOTE_MAKES: &[(&str, &str)] = &[
    ("canon", "Canon"),
    ("nikon", "Nikon"),
    ("sony", "Sony"),
    ("fujifilm", "Fujifilm"),
    ("olympus", "Olympus"),
    ("om digital", "OM Digital Solutions"),
    ("panasonic", "Panasonic"),
    ("pentax", "Pentax"),
    ("ricoh", "Ricoh"),
    ("apple", "Apple"),
    ("samsung", "Samsung"),
    ("google", "Google"),
    ("huawei", "Huawei"),
    ("xiaomi", "Xiaomi"),
];

pub fn metadata(p: &ParsedImage) -> Vec<Signal> {
    let md = &p.metadata;
    let mut v = vec![];

    // ---- editors
    let mut sources: Vec<(String, String)> = vec![]; // (value, source)
    for t in &md.tags {
        let src = match (t.group.as_str(), t.name.as_str()) {
            ("IFD0", "Software") => "EXIF Software",
            ("XMP", "xmp:CreatorTool") => "XMP CreatorTool",
            ("XMP", "photoshop:History") => "XMP photoshop:History",
            ("IPTC", "OriginatingProgram") => "IPTC OriginatingProgram",
            ("COM", "Comment") => "JPEG comment",
            ("PNG-tEXt" | "PNG-zTXt" | "PNG-iTXt", "Software") => "PNG text Software",
            _ => continue,
        };
        sources.push((t.value.clone(), src.to_string()));
    }
    for e in &md.xmp_history {
        if !e.software_agent.is_empty() {
            sources.push((e.software_agent.clone(), "XMP history".into()));
        }
    }
    let rank = |s: &str| {
        [
            "EXIF Software",
            "XMP CreatorTool",
            "PNG text Software",
            "IPTC OriginatingProgram",
            "XMP photoshop:History",
            "XMP history",
            "JPEG comment",
        ]
        .iter()
        .position(|x| *x == s)
        .unwrap_or(99)
    };
    let mut families: Vec<(&str, String, Vec<String>, usize)> = vec![];
    for (val, src) in &sources {
        if let Some(name) = editor_of(val) {
            match families.iter_mut().find(|f| f.0 == name) {
                Some(f) => {
                    if !f.2.contains(src) {
                        f.2.push(src.clone());
                    }
                    // the value shown comes from the most specific source (sources are ranked below)
                    if rank(src) < f.3 {
                        f.1 = val.clone();
                        f.3 = rank(src);
                    }
                }
                None => families.push((name, val.clone(), vec![src.clone()], rank(src))),
            }
        }
    }
    for (name, val, srcs, _) in families {
        v.push(
            sig("EXIF_SOFTWARE_EDITOR")
                .with("software", crate::parse::cap(&val, 120))
                .with("source", srcs.join(", "))
                .with("editor", name),
        );
    }

    // ---- history
    if !md.xmp_history.is_empty() {
        let last = md
            .xmp_history
            .iter()
            .rev()
            .find(|e| !e.software_agent.is_empty())
            .map(|e| e.software_agent.clone())
            .unwrap_or_else(|| "unknown software".into());
        let actions: Vec<&str> = md.xmp_history.iter().map(|e| e.action.as_str()).collect();
        v.push(
            sig("XMP_EDIT_HISTORY")
                .with("events", md.xmp_history.len().to_string())
                .with("last_agent", last)
                .with("actions", actions.join(", ")),
        );
    }

    // ---- dates
    let mut details: Vec<String> = vec![];
    let dt = md.get("IFD0", "DateTime");
    let dto = md.get("Exif", "DateTimeOriginal");
    let pdt = dt.and_then(parse_date);
    let pdto = dto.and_then(parse_date);
    if let (Some((a, _)), Some((o, _))) = (pdt, pdto) {
        if a < o {
            details.push(format!(
                "the file change date (EXIF DateTime {}) is earlier than the capture date (DateTimeOriginal {})",
                dt.unwrap_or_default(),
                dto.unwrap_or_default()
            ));
        } else if a - o > 3600 && md.get("IFD0", "Software").is_some() {
            details.push(format!(
                "EXIF DateTime {} is {} after DateTimeOriginal {} and a Software tag is present",
                dt.unwrap_or_default(),
                hours(a - o),
                dto.unwrap_or_default()
            ));
        }
    }
    let ot = md.get("Exif", "OffsetTimeOriginal").and_then(parse_offset);
    let od = md.get("Exif", "OffsetTimeDigitized").and_then(parse_offset);
    if let (Some(a), Some(b)) = (ot, od)
        && a != b
    {
        details.push(format!(
            "OffsetTimeOriginal {} differs from OffsetTimeDigitized {}",
            md.get("Exif", "OffsetTimeOriginal").unwrap_or_default(),
            md.get("Exif", "OffsetTimeDigitized").unwrap_or_default()
        ));
    }
    if let Some(time) = md.get("PNG-tIME", "tIME")
        && let Some((t, _)) = parse_date(time)
    {
        // tIME is UTC; EXIF dates are local: allow ±14 h of time zone
        if let Some((a, _)) = pdt {
            if (t - a).abs() > 14 * 3600 {
                details.push(format!(
                    "PNG tIME {time} (UTC) and EXIF DateTime {} are {} apart",
                    dt.unwrap_or_default(),
                    hours((t - a).abs())
                ));
            }
        } else if let Some((o, _)) = pdto
            && (t - o).abs() > 14 * 3600
        {
            details.push(format!(
                "PNG tIME {time} (UTC) and EXIF DateTimeOriginal {} are {} apart",
                dto.unwrap_or_default(),
                hours((t - o).abs())
            ));
        }
    }
    if let (Some(x), Some((a, _))) = (md.get("XMP", "xmp:ModifyDate"), pdt)
        && let Some((m, off)) = parse_date(x)
    {
        let exif_off = md.get("Exif", "OffsetTime").and_then(parse_offset);
        let diff = match (off, exif_off) {
            (Some(o1), Some(o2)) => (m - o1) - (a - o2),
            _ => m - a,
        };
        if diff.abs() > 60 {
            details.push(format!(
                "XMP ModifyDate {x} and EXIF DateTime {} differ by {}",
                dt.unwrap_or_default(),
                hours(diff.abs())
            ));
        }
    }
    if !details.is_empty() {
        let mut d = details.join("; ");
        if let Some(c) = d.get(..1) {
            d = c.to_uppercase() + &d[1..];
        }
        v.push(sig("METADATA_DATES_INCONSISTENT").with("details", d));
    }

    // ---- dimensions
    let px = md.get("Exif", "PixelXDimension").and_then(leading_u32);
    let py = md.get("Exif", "PixelYDimension").and_then(leading_u32);
    if let (Some(x), Some(y)) = (px, py)
        && p.width > 0
        && p.height > 0
        && x > 0
        && y > 0
    {
        let orient = md
            .get("IFD0", "Orientation")
            .and_then(leading_u32)
            .unwrap_or(1);
        let same = x == p.width && y == p.height;
        let swapped = (5..=8).contains(&orient) && x == p.height && y == p.width;
        if !same && !swapped {
            v.push(
                sig("EXIF_DIMENSIONS_MISMATCH")
                    .with("exif", format!("{x}×{y}"))
                    .with("actual", format!("{}×{}", p.width, p.height)),
            );
        }
    }

    // ---- stripped
    let has_frame = match p.format {
        Format::Jpeg => p.jpeg.as_ref().is_some_and(|j| !j.sof.is_empty()),
        Format::Heif => p.heif.is_some(),
        _ => false,
    };
    if has_frame
        && md.get("IFD0", "Make").is_none()
        && md.get("IFD0", "Model").is_none()
        && md.get("Exif", "DateTimeOriginal").is_none()
    {
        v.push(sig("METADATA_STRIPPED").with(
            "format",
            if p.format == Format::Jpeg {
                "JPEG"
            } else {
                "HEIF"
            },
        ));
    }

    // ---- messenger traces (metadata only; conservative)
    if let Some(si) = md.get("IPTC", "SpecialInstructions")
        && si.starts_with("FBMD")
    {
        v.push(sig("MESSENGER_TRACES").with("app", "Facebook").with(
            "evidence",
            "the IPTC Special Instructions field begins with \"FBMD\"",
        ));
    }

    // ---- makernote
    if let Some(make) = md.get("IFD0", "Make")
        && !md.makernote_present
    {
        let l = make.to_lowercase();
        if let Some((_, name)) = MAKERNOTE_MAKES.iter().find(|(k, _)| l.contains(k)) {
            v.push(
                sig("MAKERNOTE_MISSING_FOR_MAKE")
                    .with("make", make.to_string())
                    .with("manufacturer", *name),
            );
        }
    }

    // ---- problems
    if !md.problems.is_empty() {
        let shown: Vec<&str> = md.problems.iter().take(8).map(String::as_str).collect();
        let mut t = shown.join("; ");
        if md.problems.len() > 8 {
            t.push_str(&format!("; and {} more", md.problems.len() - 8));
        }
        v.push(sig("METADATA_PROBLEMS").with("problems", t));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HistoryEvent, JpegInfo, Tag};

    fn img(tags: &[(&str, &str, &str)]) -> ParsedImage {
        let mut p = crate::parse::empty(Format::Jpeg);
        p.width = 1200;
        p.height = 900;
        p.jpeg = Some(JpegInfo {
            sof: "SOF0".into(),
            ..Default::default()
        });
        for (g, n, v) in tags {
            p.metadata.tags.push(Tag {
                group: g.to_string(),
                id: 0,
                name: n.to_string(),
                value: v.to_string(),
            });
        }
        p
    }
    fn codes(v: &[Signal]) -> Vec<&str> {
        v.iter().map(|s| s.code.as_str()).collect()
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("2024:03:10 12:00:00").unwrap().1, None);
        let (a, o) = parse_date("2024-03-10T12:00:00.25+01:00").unwrap();
        assert_eq!(o, Some(3600));
        assert_eq!(a, parse_date("2024:03:10 12:00:00").unwrap().0);
        assert_eq!(
            parse_date("2024-03-11").unwrap().0 - parse_date("2024-03-10").unwrap().0,
            86400
        );
        assert!(parse_date("    :  :     :  :  ").is_none());
        assert!(parse_date("2024:13:10 12:00:00").is_none());
        assert_eq!(parse_offset("-05:30"), Some(-(5 * 3600 + 1800)));
        // leap year
        assert_eq!(
            parse_date("2024-03-01").unwrap().0 - parse_date("2024-02-28").unwrap().0,
            2 * 86400
        );
    }

    #[test]
    fn editors() {
        assert_eq!(editor_of("GIMP 2.10.36"), Some("GIMP"));
        assert_eq!(
            editor_of("Adobe Photoshop 25.1 (Windows)"),
            Some("Adobe Photoshop")
        );
        assert_eq!(editor_of("Paint"), Some("Microsoft Paint"));
        assert_eq!(editor_of("Paint.NET 5.0"), Some("Paint.NET"));
        assert_eq!(editor_of("HTML canvas"), None);
        assert_eq!(editor_of("Canva"), Some("Canva"));
        assert_eq!(editor_of("Firmware Version 1.0.2"), None);
        assert_eq!(editor_of("17.1.2"), None);
    }

    #[test]
    fn metadata_signal_set() {
        let mut p = img(&[
            ("IFD0", "Make", "Canon"),
            ("IFD0", "Software", "GIMP 2.10.36"),
            ("IFD0", "DateTime", "2024:03:12 10:00:00"),
            ("Exif", "DateTimeOriginal", "2024:03:10 09:00:00"),
            ("Exif", "PixelXDimension", "4000"),
            ("Exif", "PixelYDimension", "3000"),
            ("XMP", "xmp:CreatorTool", "GIMP"),
            ("XMP", "xmp:ModifyDate", "2024-03-12T10:00:00"),
            ("IPTC", "SpecialInstructions", "FBMD01000a0c0300"),
        ]);
        p.metadata.xmp_history.push(HistoryEvent {
            action: "saved".into(),
            software_agent: "Gimp 2.10 (Linux)".into(),
            ..Default::default()
        });
        p.metadata.problems.push("x".into());
        let s = metadata(&p);
        let mut c = codes(&s);
        c.sort();
        assert_eq!(
            c,
            [
                "EXIF_DIMENSIONS_MISMATCH",
                "EXIF_SOFTWARE_EDITOR",
                "MAKERNOTE_MISSING_FOR_MAKE",
                "MESSENGER_TRACES",
                "METADATA_DATES_INCONSISTENT",
                "METADATA_PROBLEMS",
                "XMP_EDIT_HISTORY"
            ]
        );
        let ed = s.iter().find(|s| s.code == "EXIF_SOFTWARE_EDITOR").unwrap();
        assert_eq!(ed.params["software"], "GIMP 2.10.36");
        assert_eq!(
            ed.params["source"],
            "EXIF Software, XMP CreatorTool, XMP history"
        );
        let d = s
            .iter()
            .find(|s| s.code == "METADATA_DATES_INCONSISTENT")
            .unwrap();
        assert!(
            d.params["details"].contains("2.0 days"),
            "{}",
            d.params["details"]
        );
        for s in &s {
            assert_eq!(s.severity, severity(&s.code));
        }
    }

    #[test]
    fn rotation_allows_swapped_dimensions_and_stripped() {
        let p = img(&[
            (
                "IFD0",
                "Orientation",
                "6 (row 0 at right and column 0 at top)",
            ),
            ("Exif", "PixelXDimension", "900"),
            ("Exif", "PixelYDimension", "1200"),
        ]);
        assert_eq!(codes(&metadata(&p)), ["METADATA_STRIPPED"]);
    }

    #[test]
    fn date_rules() {
        // DateTime earlier than original
        let p = img(&[
            ("IFD0", "Make", "X"),
            ("IFD0", "DateTime", "2024:03:09 12:00:00"),
            ("Exif", "DateTimeOriginal", "2024:03:10 12:00:00"),
        ]);
        assert_eq!(codes(&metadata(&p)), ["METADATA_DATES_INCONSISTENT"]);
        // later by 2 h without Software: fine
        let p = img(&[
            ("IFD0", "Make", "X"),
            ("IFD0", "DateTime", "2024:03:10 14:00:00"),
            ("Exif", "DateTimeOriginal", "2024:03:10 12:00:00"),
        ]);
        assert!(metadata(&p).is_empty());
        // offsets
        let p = img(&[
            ("IFD0", "Make", "X"),
            ("Exif", "OffsetTimeOriginal", "+01:00"),
            ("Exif", "OffsetTimeDigitized", "+02:00"),
        ]);
        assert_eq!(codes(&metadata(&p)), ["METADATA_DATES_INCONSISTENT"]);
        // XMP ModifyDate with zones equal in UTC
        let p = img(&[
            ("IFD0", "Make", "X"),
            ("IFD0", "DateTime", "2024:03:10 12:00:00"),
            ("Exif", "OffsetTime", "+01:00"),
            ("XMP", "xmp:ModifyDate", "2024-03-10T11:00:00Z"),
        ]);
        assert!(metadata(&p).is_empty());
    }

    #[test]
    fn marker_order_rules() {
        let mut p = img(&[("IFD0", "Make", "Canon")]);
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        p.jpeg.as_mut().unwrap().markers = mk(&[
            "SOI",
            "APP1/Exif",
            "DQT",
            "DQT",
            "SOF0",
            "DHT",
            "SOS",
            "EOI",
        ]);
        assert!(jpeg_structure(&p, 0).is_empty());
        p.jpeg.as_mut().unwrap().markers = mk(&[
            "SOI",
            "DQT",
            "APP0/JFIF",
            "APP1/Exif",
            "SOF0",
            "DHT",
            "SOS",
            "DHT",
            "SOS",
            "EOI",
        ]);
        let s = jpeg_structure(&p, 0);
        assert_eq!(codes(&s), ["JPEG_MARKER_ORDER_UNUSUAL"]);
        assert_eq!(
            s[0].params["sequence"],
            "SOI, DQT, APP0/JFIF, APP1/Exif, SOF0, DHT, SOS, …, EOI"
        );
        assert_eq!(s[0].params["expected"].matches("; ").count(), 2);
    }

    #[test]
    fn png_rules() {
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(
            png_order_problems(&c(&["IHDR", "iCCP", "PLTE", "IDAT", "IDAT", "IEND"])).is_empty()
        );
        let p = png_order_problems(&c(&[
            "IHDR", "PLTE", "iCCP", "IDAT", "tEXt", "IDAT", "tIME", "tIME", "IEND",
        ]));
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(png_order_problems(&c(&["IEND"])).len() >= 2);
    }
}
