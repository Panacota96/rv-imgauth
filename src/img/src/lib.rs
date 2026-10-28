//! RV ImgAuth image layer: parsers, metadata, pixel decoding and the analyses that produce signals of
//! modification for JPEG, HEIC/HEIF, PNG and WebP. Pure Rust, no unsafe code, no I/O and no networking: it only
//! reads the bytes it is given. The core (`rv_imgauth_core::imgscan`) calls it, in a worker process when the run
//! isolates items, maps every [`Signal`] to a finding and writes every [`MapImage`] to the item's `maps/` folder.
//!
//! ```text
//! parse::      (agent A)  bytes ─► ParsedImage (structure tree, metadata, per-format info, thumbnail)
//!                         bytes ─► ComponentCoefficients (baseline JPEG, quantized DCT)
//!                         ParsedImage ─► structure + metadata signals
//! analysis::   (agent B)  bytes ─► Pixels;  QT fingerprint; double compression; ELA; noise; JPEG ghosts;
//!                         copy-move; thumbnail comparison; reference comparison
//! ```
//! FROZEN API: the functions below keep their signatures; modules behind them may grow.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod model;
pub mod parse;

pub use model::*;

/// Parses any supported image (format detected from its bytes). Never panics on hostile input.
pub fn parse(data: &[u8]) -> Result<ParsedImage, String> {
    parse::parse(data)
}

/// Kept for the fuzz target and older callers: parse and discard the result.
pub fn parse_any(data: &[u8]) -> Result<(), String> {
    parse(data).map(|_| ())
}

/// Quantized DCT coefficients of a baseline (sequential Huffman) JPEG. Progressive and arithmetic-coded files
/// return `Err("unsupported: …")` and the coefficient-based analyses are skipped with a finding.
pub fn decode_coefficients(data: &[u8]) -> Result<Vec<ComponentCoefficients>, String> {
    parse::jpeg::decode_coefficients(data)
}

/// Signals from the file structure (marker/chunk/box order, trailing data, flags…).
pub fn structure_signals(p: &ParsedImage) -> Vec<Signal> {
    parse::signals::structure(p)
}

/// Signals from metadata (editor software, edit history, inconsistent dates and sizes, stripped metadata,
/// messenger traces, MakerNote expectations).
pub fn metadata_signals(p: &ParsedImage) -> Vec<Signal> {
    parse::signals::metadata(p)
}

/// RGB pixels of a JPEG, PNG or WebP (HEIF never: structure only in v1).
pub fn decode_pixels(data: &[u8], format: Format) -> Result<Pixels, String> {
    analysis::pixels::decode(data, format)
}
