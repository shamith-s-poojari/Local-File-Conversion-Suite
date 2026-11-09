#![no_main]
//! Spec section 6: the sniffer must not panic on any input.
//! Run with `cargo +nightly fuzz run sniff` from `crates/sniffer`.

use libfuzzer_sys::fuzz_target;

const NAMES: &[&str] = &["a.pdf", "a.docx", "a.txt", "a.md", "a.svg", "a", "a.heic"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, bytes)) = data.split_first() else {
        return;
    };
    let name = NAMES[usize::from(pick) % NAMES.len()];
    let _ = converter_sniffer::detect_bytes(name, bytes);
});
