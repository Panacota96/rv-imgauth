//! STUB (agent B): Error Level Analysis map.
use crate::model::{MapImage, Pixels};

pub fn map(_p: &Pixels, _quality: u8) -> Result<MapImage, String> {
    Err("ELA not implemented yet".into())
}
