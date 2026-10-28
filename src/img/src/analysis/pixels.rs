//! STUB (agent B): pixel decoding (zune-jpeg, png, image-webp) to RGB8.
use crate::model::{Format, Pixels};

pub fn decode(_data: &[u8], _format: Format) -> Result<Pixels, String> {
    Err("pixel decoding not implemented yet".into())
}
