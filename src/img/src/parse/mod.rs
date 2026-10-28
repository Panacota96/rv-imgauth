//! Parsers (agent A): JPEG markers + baseline Huffman decoding to coefficients, PNG chunks, WebP RIFF chunks,
//! HEIF/ISOBMFF boxes, EXIF/XMP/IPTC metadata, and the structure/metadata signals.

pub mod heif;
pub mod jpeg;
pub mod meta;
pub mod png;
pub mod signals;
pub mod webp;

use crate::model::{Format, ParsedImage};

pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    match Format::sniff(data) {
        Format::Jpeg => jpeg::parse(data),
        Format::Png => png::parse(data),
        Format::Webp => webp::parse(data),
        Format::Heif => heif::parse(data),
        Format::Unknown => Err("not a JPEG, PNG, WebP or HEIF file".into()),
    }
}
