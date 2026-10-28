//! STUB (agent B): IJG tables (T.81 Annex K + libjpeg scaling), quality estimate, table fingerprint.
use crate::model::{JpegInfo, QtFingerprint};

/// None when the file has no luminance table.
pub fn fingerprint(_j: &JpegInfo) -> Option<QtFingerprint> {
    None
}
