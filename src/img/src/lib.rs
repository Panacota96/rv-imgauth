//! RV ImgAuth image layer: parsers for the image formats the tool examines (JPEG, HEIC, PNG, WebP).
//!
//! Pure Rust, no unsafe code, no I/O and no networking: it only reads the bytes it is given. The core
//! (`rv_imgauth_core::imgscan`) calls it, in a worker process when the run isolates items.
//!
//! This build only provides the entry point; the format parsers are planned.

#![forbid(unsafe_code)]

/// Parses any supported image (format detected from its bytes).
///
/// Never panics on hostile input. Returns `Err` with a short reason when the bytes cannot be parsed —
/// in this build always `"not implemented"`.
pub fn parse_any(data: &[u8]) -> Result<(), String> {
    // TODO(img): detect JPEG / HEIC / PNG / WebP and return the parsed structure.
    let _ = data;
    Err("not implemented".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_any_is_not_implemented_yet() {
        assert_eq!(
            parse_any(b"\xFF\xD8\xFF\xE0"),
            Err("not implemented".into())
        );
        assert_eq!(parse_any(b""), Err("not implemented".into()));
    }
}
