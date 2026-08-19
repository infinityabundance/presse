//! Fuzz target for the press pipeline: arbitrary bytes → parse → compress →
//! serialize. The fuzzer feeds malformed/truncated PDFs and any panic in the
//! parse, image re-encode, or writer path is a finding.
//!
//! Run with: `cargo fuzz run fuzz_press -- -max_total_time=300 -timeout=30`
#![no_main]

use libfuzzer_sys::fuzz_target;

use presse::pdf::images::compress_images;
use presse::pdf::writer::compress_and_save_pdf;

const OUT: &str = "/tmp/presse-fuzz-out.pdf";

fuzz_target!(|data: &[u8]| {
    // Guard the decompression bomb: cap what we're willing to parse.
    if data.len() > 4 * 1024 * 1024 {
        return;
    }
    let Ok(mut doc) = lopdf::Document::load_mem(data) else {
        return; // rejected input — not a crash
    };
    compress_images(&mut doc, 50, false);
    let _ = compress_and_save_pdf(&mut doc, OUT, false);
});
