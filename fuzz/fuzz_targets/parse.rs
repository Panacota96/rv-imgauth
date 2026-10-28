#![no_main]
//! `parse()`, the signal functions and `decode_coefficients()` must never panic, overflow the stack or hang on
//! any input.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(p) = rv_imgauth_img::parse(data) {
        let _ = rv_imgauth_img::structure_signals(&p);
        let _ = rv_imgauth_img::metadata_signals(&p);
    }
    let _ = rv_imgauth_img::decode_coefficients(data);
});
