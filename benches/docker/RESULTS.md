# 100-PDF benchmark results

Measured on a 16-core machine with the release build
(`RUSTFLAGS="-C target-cpu=native"`), `presse press -q 50`.

## Corpus

100 public real-world PDFs (≈300 MB, 5,268 pages):

| category | count | source |
|---|---|---|
| research papers | 32 | arXiv (image- and text-heavy, 200 KB–36 MB) |
| government forms | 29 | IRS.gov forms + instructions |
| specs / reports | 6 | PDF 1.7 & 1.4 references, Unicode 15.0, PyMuPDF guide, Census ACS, Adobe sample |
| real-world tests | 33 | mozilla/pdf.js test corpus (books, plans, forms, XFA, scans, stress cases) |

Reproduce the corpus with `benches/docker/fetch_batch.sh` and a sparse clone of
`https://github.com/mozilla/pdf.js` (`git sparse-checkout set test/pdfs`).

## Methodology

- One run per PDF per tool: `presse press -q 50` (parallel), the same with
  `RAYON_NUM_THREADS=1` (serial baseline), and
  `gs -sDEVICE=pdfwrite -dPDFSETTINGS=/ebook`.
- Sizes measured on the produced files; reduction = `(1 − out/in) × 100`.
- Raw timings: `benches/docker/bench3.py` (resumable CSV).

## Wall time

| tool | mean | median | total (100 PDFs) |
|---|---|---|---|
| presse parallel (rayon) | 1.06 s | 16 ms | 106 s |
| presse serial (1 thread) | 1.52 s | 40 ms | 152 s |
| ghostscript /ebook | 3.22 s | 144 ms | 319 s |

Parallel vs serial: **mean 2.45×, median 1.57×** — ghostscript: **mean 177×,
median 7.6×** (total wall time 3× faster than gs).

Speedup scales with image count (the parallelized phase):

| images in doc | docs | mean speedup |
|---|---|---|
| 0 | 43 | 1.5× (noise) |
| 1–2 | 17 | 1.4× |
| 3–9 | 7 | 3.2× |
| 10+ | 33 | 4.0× |

Largest single-doc win: 7.8× (a 36 MB image-heavy arXiv paper). Documents
whose runtime is dominated by object-tree repacking (e.g. the 127k-object PDF
spec) show ~1× — the image phase is a small fraction of their time.

## Size reduction

| tool | mean | median |
|---|---|---|
| presse (parallel = serial, deterministic) | 19.5 % | 9.1 % |
| ghostscript /ebook | 60.5 % | 64.8 % |

presse only re-encodes image streams (JPEG q50) and repacks the object tree;
ghostscript `/ebook` additionally downsamples images and re-compresses all
streams, so it shrinks files more on this corpus. presse's advantage is speed,
not size.

## Correctness gates on all 100 outputs

- **qpdf `--check`**: 96/100 fully clean. The 4 exceptions are source-PDF
  quirks passed through (3× "input stream is complete but output may still be
  valid"; 1× page-tree loop in a deliberately malformed pdf.js regression
  file that poppler already flags on the *original*).
- **Visual sweep** (`benches/docker/visual_sweep.py`, pdftoppm render + SSIM
  + luminance-ratio heuristics, all pages capped at 40 for long docs):
  **98/100 at SSIM ≥ 0.99** (97 at exactly 1.000). The two exceptions were
  both reproduced with a bare `lopdf::Document::load` + `save` (zero presse
  code) — pre-existing lopdf parse artifacts on pathological sources, not
  regressions from the compression pipeline.

One corpus candidate (a Brotli-compressed file) cannot be loaded by lopdf at
all (`BrotliDecode` unsupported) and is skipped with a warning.
