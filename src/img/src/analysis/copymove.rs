//! STUB (agent B): block-DCT copy-move detection.
use super::CopyMoveOptions;
use crate::model::{MapImage, Pixels, Signal};

pub fn detect(_p: &Pixels, _o: &CopyMoveOptions) -> (Vec<Signal>, Option<MapImage>) {
    (vec![], None)
}
