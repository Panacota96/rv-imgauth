//! STUB (agent B): IJG tables (T.81 Annex K + libjpeg scaling), quality estimate, table fingerprint.
use crate::model::{JpegInfo, QtFingerprint};

/// None when the file has no luminance table.
pub fn fingerprint(_j: &JpegInfo) -> Option<QtFingerprint> {
    None
}

/// QT_STANDARD_IJG {quality} or QT_NON_STANDARD {nearest, distance}.
pub fn signals(_fp: &QtFingerprint) -> Vec<crate::model::Signal> {
    vec![]
}
