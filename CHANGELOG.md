# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- **Parallel image re-encoding** — image streams are detached from the
  `Document`, re-encoded concurrently with rayon on owned buffers, and
  written back in a single serial pass (document-lock-free; only the dedup
  cache is shared).
- **`--dpi` resolution cap** (`press -d <dpi>`) — placement-aware image
  downsampling with Ghostscript-style presets (75 screen, 150 ebook,
  300 printer, 600 prepress). Strict cap: never up-samples; images whose
  placement cannot be determined keep source resolution.
- **Pluggable GPU transcoders** — `--acceleration cuda|rocm` behind a
  unified `ImageTranscoder` trait with graceful per-stream CPU fallback
  (opt-in Cargo features; not linked into default builds).
- **Regression suite** — 21 tests covering structural integrity (qpdf /
  ghostscript / `/Length` gates), visual SSIM, dpi-capping invariants,
  cross-reference reachability, grayscale component preservation,
  serial/parallel determinism, gapped numbering, and GPU fallback.
- **Benchmark + quality harness** — containerized 100-PDF benchmark,
  quality-vs-speed analysis (including MuPDF and the non-compressing Rust
  tools), a dpi sweep, and a multi-tool Pareto-frontier harness.
- `mimalloc` on the musl release artifact; `zlib-rs` deflate backend;
  memory-mapped input reads; deduplicated image re-encodes.

### Fixed

- **Cross-reference correctness** — objects are renumbered before saving,
  closing lopdf's gapped-numbering xref truncation (blank pages on large
  real-world PDFs; upstream tracked at J-F-Liu/lopdf#558).
- **Grayscale JPEG corruption** — `/DeviceGray` streams keep a
  single-component JPEG instead of a 3-component one that rendered as
  washed-out garbage.
- **Transcode cache** — keyed by quality, input kind, dimensions, and
  exact pixel bytes (equality on bytes, never a hash alone); a per-entry
  `OnceLock` guarantees one encode per unique image even under contention.

## [0.3.2] - previous release

Structural compression, image recompression, merging, conversion (see
`git log` for details).
