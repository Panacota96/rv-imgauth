//! STUB (agent B): embedded thumbnail vs main image.
use crate::model::{MapImage, Pixels, Signal};

pub fn compare(_thumbnail: &[u8], _main: &Pixels) -> (Vec<Signal>, Option<MapImage>) {
    (vec![], None)
}
