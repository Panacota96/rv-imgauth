//! The image model shared by the parsers (agent A) and the analyses (agent B). FROZEN: fields may be added with a
//! default, never renamed or removed. No dependency on rv-imgauth-core: signals are mapped to findings by core.
//!
//! ```text
//!  bytes ──► parse()  ──► ParsedImage { format, structure tree, metadata, jpeg/png/webp/heif info, thumbnail }
//!        ──► decode_coefficients() (JPEG baseline) ──► Vec<ComponentCoefficients>
//!        ──► decode_pixels() (JPEG/PNG/WebP; HEIC never here) ──► Pixels (RGB8)
//!  analyses(ParsedImage, coefficients, pixels) ──► Vec<Signal> + Vec<MapImage>
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Png,
    Webp,
    Heif,
    Unknown,
}

impl Format {
    /// From the first bytes (magic numbers; HEIF by `ftyp` brand).
    pub fn sniff(data: &[u8]) -> Format {
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Format::Jpeg
        } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Format::Png
        } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            Format::Webp
        } else if data.len() >= 12 && &data[4..8] == b"ftyp" {
            let brand = &data[8..12];
            if [
                b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1", b"avif",
            ]
            .iter()
            .any(|b| brand == *b)
            {
                Format::Heif
            } else {
                Format::Unknown
            }
        } else {
            Format::Unknown
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Heif => "heif",
            Format::Unknown => "unknown",
        }
    }
}

/// One node of the structure tree: a JPEG marker segment, PNG chunk, RIFF chunk or ISOBMFF box.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// "SOI", "APP1/Exif", "DQT", "IHDR", "VP8X", "ftyp", "meta/iinf", …
    pub kind: String,
    /// Byte offset of the segment start in the file.
    pub offset: u64,
    /// Total length in bytes including header.
    pub length: u64,
    /// Small facts about the segment (strings only; e.g. "id" => "0", "brand" => "heic").
    pub detail: BTreeMap<String, String>,
    pub children: Vec<Segment>,
}

/// A raw structural observation the parser made; turned into findings by `structure_signals`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    /// Finding code (worded in i18n): e.g. "JPEG_MARKER_ORDER_UNUSUAL".
    pub code: String,
    /// "info" | "low" | "medium" | "high"
    pub severity: String,
    /// Named parameters for the i18n text (strings only).
    pub params: BTreeMap<String, String>,
}

impl Signal {
    pub fn new(code: &str, severity: &str) -> Signal {
        Signal {
            code: code.into(),
            severity: severity.into(),
            params: BTreeMap::new(),
        }
    }
    pub fn with(mut self, k: &str, v: impl Into<String>) -> Signal {
        self.params.insert(k.into(), v.into());
        self
    }
}

/// One metadata value, as read (value rendered as text; raw kept for EXIF rational/undefined types).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// "IFD0", "Exif", "GPS", "IFD1", "Interop", "XMP", "IPTC", "PNG-tEXt", "PNG-iTXt", "PNG-tIME", "WebP", "HEIF"
    pub group: String,
    /// Numeric tag id where one exists (EXIF, IPTC dataset as record*1000+dataset), else 0.
    pub id: u32,
    /// Human-readable name ("DateTimeOriginal", "xmp:CreatorTool", "Software", …).
    pub name: String,
    pub value: String,
}

/// One xmpMM:History stEvt entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryEvent {
    pub action: String,
    pub when: String,
    pub software_agent: String,
    pub changed: String,
    pub instance_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub tags: Vec<Tag>,
    /// Raw XMP packet (UTF-8) when present.
    pub xmp: Option<String>,
    pub xmp_history: Vec<HistoryEvent>,
    /// SHA-256 of the ICC profile bytes, when present.
    pub icc_sha256: Option<String>,
    /// True when the EXIF MakerNote tag is present (its layout is not parsed in v1).
    pub makernote_present: bool,
    /// Problems met while reading metadata (broken IFD offsets, truncated packets…), as text.
    pub problems: Vec<String>,
}

impl Metadata {
    /// First value of a tag by name in a group ("Exif", "DateTimeOriginal").
    pub fn get(&self, group: &str, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.group == group && t.name == name)
            .map(|t| t.value.as_str())
    }
}

/// A JPEG quantization table in NATURAL (row-major) order, de-zigzagged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuantTable {
    /// Table id 0..3 (Tq).
    pub id: u8,
    /// 0 = 8-bit, 1 = 16-bit entries.
    pub precision: u8,
    pub values: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HuffmanTable {
    /// 0 = DC, 1 = AC.
    pub class: u8,
    pub id: u8,
    /// BITS: number of codes of each length 1..16.
    pub counts: Vec<u8>,
    pub symbols: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: u8,
    pub h: u8,
    pub v: u8,
    /// Quantization table id.
    pub tq: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JpegInfo {
    /// "SOF0" baseline, "SOF1", "SOF2" progressive, …
    pub sof: String,
    pub precision: u8,
    pub width: u32,
    pub height: u32,
    pub components: Vec<Component>,
    /// Every DQT table in file order (a duplicate id appears twice).
    pub qtables: Vec<QuantTable>,
    pub huffman: Vec<HuffmanTable>,
    pub restart_interval: u16,
    /// Marker sequence as written ("SOI","APP0/JFIF","APP1/Exif","DQT",…,"EOI").
    pub markers: Vec<String>,
    /// "4:4:4", "4:2:2", "4:2:0", "4:4:0", "4:1:1", "grey", "other".
    pub subsampling: String,
    /// Bytes after EOI (MPF secondary images or trailers).
    pub after_eoi: u64,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    /// Chunk types in file order.
    pub chunks: Vec<String>,
    pub after_iend: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WebpInfo {
    /// "VP8", "VP8L" or "VP8X".
    pub kind: String,
    pub width: u32,
    pub height: u32,
    pub chunks: Vec<String>,
    /// VP8X flags as names: "ICC", "ALPHA", "EXIF", "XMP", "ANIMATION".
    pub flags: Vec<String>,
    pub lossless: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeifItem {
    pub id: u32,
    /// "hvc1", "grid", "Exif", "mime", "jpeg", "av01", …
    pub item_type: String,
    pub hidden: bool,
    /// ispe width/height when declared.
    pub width: u32,
    pub height: u32,
    /// Item references from this item: ("thmb", to_id), ("cdsc", to_id), ("dimg", to_id), ("auxl", to_id).
    pub refs: Vec<(String, u32)>,
    /// (offset, length) extents in the file.
    pub extents: Vec<(u64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeifInfo {
    pub major_brand: String,
    pub compatible_brands: Vec<String>,
    pub primary_item: u32,
    pub items: Vec<HeifItem>,
    /// Grid of the primary item when it is a grid: (rows, columns, output width, output height).
    pub grid: Option<(u32, u32, u32, u32)>,
    pub width: u32,
    pub height: u32,
    /// Bytes of the file covered by no box header and no item extent.
    pub unreferenced_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParsedImage {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub structure: Vec<Segment>,
    pub metadata: Metadata,
    pub jpeg: Option<JpegInfo>,
    pub png: Option<PngInfo>,
    pub webp: Option<WebpInfo>,
    pub heif: Option<HeifInfo>,
    /// Embedded thumbnail as encoded bytes (EXIF IFD1 JPEG, HEIF thumbnail item if JPEG), when present.
    #[serde(skip)]
    pub thumbnail: Option<Vec<u8>>,
    /// Structural observations for `structure_signals` (filled by the parser).
    pub signals: Vec<Signal>,
}

/// Quantized DCT coefficients of one JPEG component, block by block (each block 64 values in NATURAL order).
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentCoefficients {
    pub component_id: u8,
    pub tq: u8,
    pub blocks_wide: u32,
    pub blocks_high: u32,
    /// blocks_wide * blocks_high * 64 quantized coefficients.
    pub coefs: Vec<i16>,
}

/// Decoded pixels, RGB 8-bit, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// A visual map written to the item's `maps/` folder.
#[derive(Clone, Debug, PartialEq)]
pub struct MapImage {
    /// File name ("ela.png", "noise.png", "noise-sigma.png", "copymove.png", "ghost-q75.png", "thumbnail-diff.png").
    pub name: String,
    pub png: Vec<u8>,
    /// Parameters that produced it (recorded in the report and audit log; strings only).
    pub params: BTreeMap<String, String>,
}

/// Fingerprint of a JPEG's quantization tables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QtFingerprint {
    /// SHA-256 over the de-zigzagged luma then chroma tables (u16 big-endian), the database key.
    pub sha256: String,
    /// Exact IJG standard quality (both tables), if any.
    pub ijg_quality: Option<u8>,
    /// Nearest IJG quality by sum of absolute differences, and that distance.
    pub nearest_quality: u8,
    pub distance: u32,
}
