#![no_main]
//! `parse_any()` must never panic, overflow the stack or hang on any input.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rv_imgauth_img::parse_any(data);
});
