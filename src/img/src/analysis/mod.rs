//! Analyses (agent B). Every function is deterministic for the same input and parameters (the parameters are
//! returned in MapImage::params / Signal::params so the report can state them).

pub mod copymove;
pub mod double;
pub mod ela;
pub mod ghosts;
pub mod noise;
pub mod pixels;
pub mod qt;
pub mod reference;
pub mod thumbnail;

/// Copy-move detection settings (defaults are the documented v1 values).
#[derive(Clone, Debug, PartialEq)]
pub struct CopyMoveOptions {
    /// Long side of the grey image the search runs on.
    pub max_side: u32,
    pub block: u32,
    /// Minimum length of a shift vector, in pixels of the downscaled image.
    pub min_shift: u32,
    /// Minimum number of matching blocks voting for one shift.
    pub min_blocks: u32,
}

impl Default for CopyMoveOptions {
    fn default() -> Self {
        CopyMoveOptions {
            max_side: 1024,
            block: 16,
            min_shift: 32,
            min_blocks: 20,
        }
    }
}
