## Highlights

This branch grew from the rayon-parallel re-encode PR into the full
image-pipeline release: compression **speed**, **fidelity knobs**, the
**measured size gaps** against qpdf / MuPDF / ghostscript, the GPU
backends, and — newest — the **`optimize`-feature codec candidates**
(JPEG2000 / JBIG2 / MRC), **font subsetting**, **generic resource dedup**
and **Zopfli**, all default-off. TL;DR:

- **Rayon-parallel image re-encoding** — image streams re-encode
  concurrently on owned buffers, one serial write-back pass; 16-core wall
  time mean **2.2× / median 1.5×** over serial on the 100-PDF corpus.
- **Cross-reference correctness** — objects renumbered to contiguous ids
  before save; closes lopdf's gapped-numbering xref truncation
  (J-F-Liu/lopdf#558) that qpdf rejects and poppler can render blank
  (756-page / 127k-object doc). The linear renumberer also repairs
  `max_id` and bookmark bookkeeping (regression: stale `max_id` + object
  streams).
- **Quality knobs** — `-d/--dpi` placement-aware resolution caps
  (75/150/300/600 presets, never upscales) and `-s/--ssim` calibrated
  fidelity targets.
- **Pareto size gaps closed** — duplicate-image coalescing (photos60 q30
  22.96 → 6.46 MB, *below* mutool's 6.76), flate-wrapped JPEG, `--palette`
  `/Indexed` candidates, `--raster-classify` 1-bit CCITT G4 masks (an
  **opaque** `DeviceGray` image — deliberately not an `/ImageMask`
  stencil, whose transparent white and current-color ink would change
  rendering), `--recompress-flate` (qpdf's structural lever), and
  `--jpeg-encoder` libjpeg-style 4:2:0 CPU codec (photos20 q50 −8.3%).
- **GPU backends** — `--acceleration cuda|rocm` behind one
  `ImageTranscoder` trait with graceful CPU fallback; the cuda backend is
  device-resident (per-slot streams, no pixel round-trips) and now decodes
  baseline 4:2:0 JPEGs on the **NVDEC hardware engine** (Video Codec SDK)
  with an embedded-PTX NV12→planar conversion kernel.
- **Perf follow-ups** — linear-time object renumbering (530 → 62 ms on a
  67k-object doc) and a bounded dedup-cache hash (first/last 4 KiB).
- **`optimize` feature (new)** — `--compression {fast,balanced,small,
  smallest}` presets and the individual default-off flags `--dedup`,
  `--zopfli`, `--font-subset`, `--jbig2`, `--jpeg2000`, `--mrc`. Every
  pass follows the pipeline's candidate/court discipline: applied only
  when strictly smaller *and* within its measured fidelity budget — the
  lossless structural rewrites by construction, the lossy candidates on a
  per-image measurement (palette ≥0.9999 native SSIM; the bitonal masks —
  G4/JBIG2/MRC — on the classifier's reconstruction-error gate, added
  after review: the RGB→bitonal step is now *verified* per image, not
  assumed safe) — and a no-op whenever a conservative gate rejects it. A
  build without the feature errors explicitly when a flag is requested.
- **Harness** — regression suite (44 tests incl. the codec gates, the post-review MRC/form/classifier regressions, and the new JPEG2000 runtime-admission regression: clean photo admitted, heavy-noise photo rejected at the same rate target),
  fuzz target (4.4M inputs clean), and a Pareto-frontier benchmark with
  per-channel SSIM witnesses.
- **Dependency documentation** — README's Dependencies section now spells
  out the install story: a Rust toolchain (1.85+, edition 2024) is the only
  build requirement; the cuda NVDEC stage needs the Video Codec SDK headers
  (`ffnvcodec-headers`) alongside the driver's `libnvcuvid.so.1`; and the
  regression/bench tooling (poppler-utils, qpdf, ghostscript, mupdf-tools,
  pngquant + tesseract-ocr-eng) is listed with a Debian/Ubuntu one-liner.
- **Pareto matrix results** (synthetic corpus, best-of-1, q50,
  72+300 dpi renders, `pareto.py --optimize`): on scanned.pdf (3.40 MB
  grainy gray scans) `--mrc` **wins the ≥0.999 frontier outright** —
  2.3 KB at render SSIM 0.9997 vs qpdf 0.36 MB / mutool 0.29 MB / presse
  q50 0.27 MB / gs `/screen` 0.48 MB at equal or lower fidelity; the
  bitonal masks own the ≤0.98 frontier (`--jbig2` 1.3 KB, `smallest`
  1.1 KB), and ≥0.9999 stays on the grain-preserving JPEG path
  (0.56–0.66 MB). On image-heavy.pdf (17.98 MB, 60 photos) `--jpeg2000`
  wins the per-image size court (1.38 MB vs 1.51 MB at q50) at render
  SSIM 0.9931 vs 0.9995 — inside the frontier, dominated by the ssim
  mode (0.28 MB @ 0.9967), and never qualifying at ≥0.9999: the expected
  honest shape for a rate-targeted lossy candidate, with the runtime
  gate doing exactly its job. `--dedup`/`--zopfli`/`--font-subset` are
  structural no-ops on this corpus; the dedup win is the existing
  photos60 Pareto row (22.96 → 6.46 MB).

---

## The `optimize` feature (default-off, behind `--features optimize`)

Build: `cargo build --release --features optimize`. The flags:

- **`--dedup`** — byte-identical non-image streams (FontFile programs, ICC
  profiles, XForms, patterns, arbitrary streams) coalesce onto one
  canonical object with every reference rewritten; identical stream +
  identical dictionary renders identically, so lossless by construction.
- **`--zopfli`** — the `--recompress-flate` pass with the Zopfli deflate
  search (few % smaller on text/content streams, strictly-smaller gate).
- **`--font-subset`** — embedded TrueType fonts are subset to the
  glyphs the content streams actually show (typst's `subsetter`) and
  rewritten as CID fonts (`/Identity-H` + identity `CIDToGIDMap`) with the
  text strings remapped in place and a rebuilt `/ToUnicode`; fonts whose
  used-glyph mapping cannot be resolved exactly (unusual encodings,
  resource-less forms, unparseable streams) and CFF (`FontFile3`)
  programs are skipped — CFF subsetting is deliberately not implemented
  (a CFF subset needs a `CIDFontType0` descendant, not the TrueType
  `CIDFontType2` installer) — and the subset is kept only when strictly
  smaller. A 194 KB font-heavy PDF → ~7 KB with pixel-identical rendering
  and intact text extraction.
- **`--jbig2`** — a lossless JBIG2 (symbol-dictionary) candidate for the
  same 1-bit masks the G4 candidate uses: repeated glyph shapes share one
  dictionary entry. The size court keeps the smaller of G4 / JBIG2 /
  original. (Poppler's JBIG2 decoder emits *inverted* samples, so JBIG2
  images use the identity `/Decode` — the opposite of G4's `[1 0]` — and
  its parser prints a benign "extraneous byte" notice on the encoder's
  spec-mandated A.3.6 flush marker; both verified pixel-identically
  against poppler/gs/mutool.)
- **`--jpeg2000`** — a JPEG2000 candidate (pure-Rust `j2k` codec) for
  continuous-tone images, emitted as a minimal JP2 file (signature /
  file-type / image-header + sRGB / codestream boxes) so poppler decodes
  it cleanly, rate-targeted at 85% of the JPEG candidate's bytes. The
  rate target is only a sizing hint: **every candidate now carries
  runtime fidelity admission** — it is decoded back (j2k's own strict
  JP2 validator caught a real bug here: our ihdr wrote `0x87` = 8-bit
  *signed*, which every spec-strict decoder rejects; now `0x07`,
  unsigned) and measured against the source pixels on the native 512-px
  window (`CandidateEvidence`: SSIM + luma/chroma/edge error), and
  admitted to the size court only above a 0.98 native-SSIM gate. This is
  the first implementation of the generic runtime candidate court every
  future lossy representation (Jpegli, textured MRC, …) is expected to
  fill — a candidate earns its place by measured reconstruction, never by
  byte-budget ratios. Ghostscript's JPX decoder is broken on *all* JP2
  files (reproduces with OpenJPEG's own output), so the regression oracle
  is poppler + mutool + OpenJPEG.
- **`--mrc`** — **flat two-tone mixed-raster content**: the composite
  commercial scan compressors use (ABBYY / LEADTOOLS / Pdftools), minus
  their textured-background layer. A solid paper-color background (the
  median paper color, emitted as a 1×1 image — deliberately *not* a JPEG,
  because near-flat JPEG bitstreams are mis-decoded as full-page
  gradients by poppler and Ghostscript), a solid ink-color foreground,
  and a high-resolution lossless CCITT G4 mask composited as the
  foreground's `/SMask` (a full image XObject — Ghostscript silently
  drops a soft mask whose stream lacks `/Type /XObject` +
  `/Subtype /Image`); the content stream is rewritten to draw background
  then foreground *without* re-applying the placement `cm` (the source
  image's transform is already current at the injection point; re-emitting
  it squares the scale). This is the *intended* home for mask compositing —
  the graphics state and underlying layers are under presse's control, so
  the composite cannot leak background through the paper or recolor the
  ink (regression-tested pixel-identically under a blue rectangle with a
  red current color, the same trap the G4 work passes). The mask is
  always G4: poppler's JBIG2 inversion would make a JBIG2 mask's polarity
  viewer-dependent. On a 3.4 MB grainy scan corpus the composite lands at
  ~2.3 KB — and "flat" is the operative word: the median paper color
  *replaces* the paper texture, which is precisely the distinction from
  future **textured / continuous-tone MRC** (compressed photo background +
  foreground + mask, the ABBYY-style mechanism). The candidate ladder is
  `source scan 3.40 MB → JPEG ~260 KB → textured MRC (future) → flat MRC
  2.3 KB → JBIG2 1.3 KB`, and the fidelity court decides where a document
  belongs. The earlier poppler "Bogus memory allocation size" notice on
  large full-bleed masked images was the double-`cm` rewrite bug
  (1600×1200 squared to 2,560,000×1,440,000, overflowing poppler's
  soft-mask allocation and dropping the foreground); with the rewrite
  fixed and the mask typed, poppler / mutool / ghostscript render the
  composite pixel-identically (5.92% ink coverage, min luma 214 at 300
  dpi — identical to the source).
- **Post-review hardening of `--mrc`** — three findings from review, all
  fixed and regression-tested: (1) the content rewrite no longer re-applies
  the placement `cm` for the foreground (it would square the scale; the
  asymmetric-transform regression — translate + rotate + shear + scale —
  asserts the ink lands exactly on the source ink, bbox within ±2 px and
  Jaccard ≥0.97 at 300 dpi); (2) the foreground resource is now registered
  in a *Form XObject's* own `/Resources` too (forms are streams, not page
  dictionaries — regression: image inside a self-contained form); (3) the
  classifier's masks are row-packed, which the G4/JBIG2/MRC consumers
  require — widths not divisible by 8 previously panicked the MRC layer
  builder.
- **`--compression`** presets: `fast` (JPEG-only), `balanced` (+`--dedup`),
  `small` (+`--zopfli`, `--recompress-flate`), `smallest` (+`--jbig2`,
  `--jpeg2000`, `--font-subset`, `--mrc` and every structural pass).
  `smallest` on the scan corpus: 3.40 MB → ~1.1 KB (JBIG2 masks,
  pixel-identical to the G4 reference); on the 18 MB photo corpus:
  1.38 MB at `-q 50` (J2K candidates admitted per-image above the 0.98
  native-SSIM runtime gate).

Measured on the deterministic corpus (best-of-1, `-q 50`, current tree;
every cell is reproduced by `benches/docker/pareto.py --optimize` — see
`QUALITY.md` for the full matrix with render-SSIM witnesses):

| corpus file | `fast` | `--mrc` | `--jbig2` | `smallest` |
|---|---|---|---|---|
| scanned.pdf (3.40 MB, 3 grainy gray pages) | 0.27 MB | 2.3 KB | 1.3 KB | 1.1 KB |
| image-heavy.pdf (17.98 MB, 60 photos, 24 pages) | 1.51 MB | — | — | 1.38 MB |

---

## What this does

**Parallel image re-encoding (`src/pdf/images.rs`).** `compress_images` runs in three phases: image streams are detached from the `Document`, re-encoded concurrently with rayon on owned buffers (document-lock-free — no `Document` state is touched from worker threads; the dedup cache is the only shared structure and is internally synchronized), and applied back in a single serial pass before serialization. Raw pixel buffers are processed in fixed 32 KiB chunks for the RGBA→RGB normalization pass; grayscale payloads keep a single-component JPEG (previously `DynamicImage` forced a 3-component JPEG into `/DeviceGray` streams, which rendered as washed-out garbage).

**Resolution caps (`-d, --dpi`) and fidelity targets (`-s, --ssim`).** `press -d <dpi>` downsamples every placed image to at most `w·dpi/72 × h·dpi/72` pixels, Ghostscript-style: 75 screen / 150 ebook / 300 printer / 600 prepress. Placement is read from the page content stream's transform matrix at each `Do` — a small content interpreter (`src/pdf/placements.rs`) tracking `q/Q/cm` with form-XObject recursion, recording the largest placement per shared image. Images whose placement cannot be parsed stay at source resolution, the cap never upscales, and `/Width` + `/Height` are rewritten with the payload. `press -s <target>` replaces the arbitrary `-q` knob with a fidelity target: the JPEG quality is derived from a committed calibration curve measured on grainy scans (the worst case for JPEG), so smoother content always *exceeds* the target. The default (no `-d`, `-s 1.0`) is byte-identical to the previous behavior.

**Candidates, not replacements.** Every non-default representation is a candidate in a size court: the pipeline encodes the JPEG (and, when enabled, the indexed, 1-bit G4, JBIG2, JPEG2000 and MRC variants), and the *strictly smallest* candidate that is smaller than the source wins per image. This is what keeps `--palette`, `--raster-classify`, `--jbig2`, `--jpeg2000` and `--mrc` safe on arbitrary inputs — a representation is never forced.

**Correctness first.** `--raster-classify`'s bitonal output is an opaque 1-bit `DeviceGray` image with `/Decode [1 0]` + `DecodeParms` — not an `/ImageMask` stencil (a stencil's white is transparent and its ink inherits the current graphics color, so it is not a substitute for an opaque raster; the "lossless" wording is confined to the G4 encoding — the RGB→bitonal conversion itself is lossy). The renumberer repairs `max_id` and bookmarks (the writer derives xref sizes and object-stream ids from `max_id`). The MRC foreground name is registered in the *owner's* resources dict (page or form) because renderers resolve page content through the page dict, not the content stream's own.

## Benchmarking

See `benches/docker/QUALITY.md`: Pareto frontier vs qpdf/MuPDF/gs/
pdf-optimizer/OCRmyPDF at equal measured fidelity (render SSIM at 72/300
dpi + a native-image 512-px-window witness), the `-d × -s` matrix, the
`--jpeg-encoder` chroma experiment, and the new `optimize`-feature
measurements. The regression suite (`tests/regression.rs`, 43 tests) and
the fuzz target run in CI.

**Validation run on this branch:** `cargo test --release --all-features`
(20 lib + 20 unit + 44 integration pass, incl. the MRC,
form-ownership, affine-transform, classifier-gate and JPEG2000
runtime-admission regressions), `cargo clippy --release --all-features
--all-targets -- -D warnings` clean, `cargo fmt --check` clean, `cargo
fuzz run fuzz_press` (7.9M inputs in this run, no findings),
`ci/validate_corpus.py` (all 10 checks pass on the fixture corpus),
and the MRC output re-rendered in poppler / mutool / ghostscript at 300
dpi with identical ink coverage. Default (no-feature) build is
warning-free with an explicit feature-guard error for the optimize flags.





