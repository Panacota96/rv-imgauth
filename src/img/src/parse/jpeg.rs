//! STUB (agent A): JPEG marker parser and baseline Huffman decoder.
use crate::model::{ComponentCoefficients, ParsedImage};

pub fn parse(_data: &[u8]) -> Result<ParsedImage, String> {
    Err("jpeg parser not implemented yet".into())
}

pub fn decode_coefficients(_data: &[u8]) -> Result<Vec<ComponentCoefficients>, String> {
    Err("jpeg coefficient decoder not implemented yet".into())
}
