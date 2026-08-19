# Quality benchmark

How output quality relates to speed for the tools measured in `RESULTS.md` —
presse (parallel), ghostscript `/ebook`, and the two Rust tools that do
**not** compress PDFs (unpdf, pdfrs/pdfcli), included so their speed can be
judged against what they actually produce.

## TL;DR

- **presse** is the only tool whose speed reflects real, full-resolution
  compression: 16–33% size reduction at SSIM ≥ 0.998, and ~7× faster than
  ghostscript on the same corpus.
- **ghostscript**'s size wins are resolution loss, not compression:
  `/ebook` downsamples images placed above 150 dpi (6483 → 1902 ppi on an
  IRS scan) and is a *no-op* on images placed at ≤ 150 dpi (photo PDFs pass
  through unchanged).
- **unpdf and pdfrs do not compress.** unpdf extracts text (0.859
  precision / 0.921 recall vs poppler) and skips images entirely (0 images
  extracted from a 20-photo PDF). pdfrs's speed is the speed of doing
  almost nothing: its `merge` drops images (28 KB blank-page output from
  48 MB of inputs) and `pdf-to-md` fails on 27/101 corpus files.

## Methodology

- Hardware/software: RTX 4080 SUPER, 16-core CPU, CUDA 13.3, Ghostscript
  10.07.1, unpdf 0.12.0, pdfrs 0.1.9 (binary `pdfcli`), poppler + qpdf for
  validation. presse built with `RUSTFLAGS="-C target-cpu=native"`.
- Commands:
  - presse: `presse press -q <30|50> -a cpu <in> -o <out>`
  - ghostscript: `gs -q -sDEVICE=pdfwrite -dPDFSETTINGS=/ebook -dNOPAUSE
    -dBATCH -sOutputFile=<out> <in>`
  - unpdf: `unpdf text <in>` (extraction; no PDF output)
  - pdfrs: `pdfcli pdf-to-md <in> <out>`, `pdfcli merge -o <out> <in>...`
- Quality metric: whole-image SSIM of 60 dpi `pdftoppm` renders (output vs
  source), plus the maximum image resolution (`pdfimages -list`, x-ppi) as
  the resolution-preservation check.
- Times: quality tables are best-of-1 per file; the speed table is the
  per-file median over the 101-file corpus from `RESULTS.md` (excluding the
  60 s `specs_pdfref17old` outlier that is a lopdf single-threaded parse
  bottleneck — parallel == serial there).

## Quality × size × time on image-bearing files

| file (source size) | presse q50 → MB / SSIM | presse q30 → MB / SSIM | gs /ebook → MB / SSIM | gs max image ppi |
|---|---|---|---|---|
| arxiv_gpt3_2005 (6.77 MB) | 5.69 / 0.9999 | 5.03 / 0.9999 | 2.04 / 0.9944 | 1234 → 1030 |
| arxiv_transformer_xl1901 (4.57 MB) | 4.41 / 1.0000 | 4.40 / 1.0000 | 1.04 / 0.9931 | 1119 → 544 |
| irs_fw2 (2.15 MB) | 2.03 / 1.0000 | 2.02 / 1.0000 | 0.18 / 0.9989 | 6483 → 1902 |
| specs_pymupdf_guide (7.11 MB) | 4.74 / 0.9999 | 4.35 / 0.9999 | 4.61 / 0.9985 | 8736 → 6422 |
| photos20 (12.08 MB) | 11.04 / 0.9981 | 7.65 / 0.9971 | 12.07 / 1.0000 (no-op) | 274 (unchanged) |

Time (seconds, best-of-1): presse 0.16–0.47 s per file across the set;
ghostscript 0.48–2.48 s (3–10× slower). On photos20 (72 dpi image
placement) ghostscript is a pass-through no-op at 0.25 s — identical size,
SSIM 1.0, because `/ebook` only downsamples images above its 150 dpi
target.

Resolution is the whole story of the size gap: presse preserves every
image's resolution (max ppi unchanged at every quality); ghostscript
downsamples scans to 544–1902 ppi. presse q30 reaches within ~9% of
ghostscript's scan sizes (e.g. irs_fw2 2.02 vs 0.18 MB — gs still smaller
because it throws away most of the pixels) while holding SSIM ≥ 0.997 at
full resolution.

## The non-compressors

Measured so their speed is not mistaken for compression throughput:

- **unpdf** (0.357 s median/file): text extraction only. Word fidelity vs
  poppler `pdftotext` on a paper PDF: 2101 unique words, 0.859 precision /
  0.921 recall. `unpdf extract` returned **0 images** from a 20-photo PDF —
  image extraction is non-functional on that input. No PDF output; no
  compression.
- **pdfrs / pdfcli** (0.029 s median/file on the 74/101 files it handles):
  `pdf-to-md` fails outright on 27/101 corpus files (0-byte/no output). Its
  `merge` produced an 80-page PDF of **28,894 bytes from 48,326,372 bytes
  of inputs — rendered blank** (the images are dropped, not compressed).
  `md-to-pdf` creates a 1.4 KB PDF from 64 B of markdown (a creator, not a
  compressor).

## Speed in context (101-file corpus, excluding the 60 s spec outlier)

| tool | operation | total | median/file |
|---|---|---|---|
| presse parallel | compress, full res | 14.3 s | 0.017 s |
| presse serial | compress | 22.1 s | 0.023 s |
| presse GPU | compress (≥ 1 MiB images) | 17.5 s | 0.020 s |
| unpdf | text extraction | 36.2 s | 0.357 s |
| pdfrs | md conversion (74/101 files) | 11.4 s | 0.029 s |
| ghostscript /ebook | compress (resolution-lossy) | 293 s | 0.152 s |

## Warm-batch note

The GPU path's 115 ms context init dominates single-shot runs. One `press`
invocation with multiple inputs shares the transcoder, so batches amortize
it (interleaved best-of-3, image-heavy set: photos20 + photos60 + mixed +
small-heavy):

| scenario | cpu-par | gpu |
|---|---|---|
| single-shot, mixed doc | 0.489 s | 0.402 s |
| 4-doc batch, one process | 1.214 s | 1.111 s |
| 8-doc batch (2× the set) | 2.502 s | 2.166 s |

Warm per-doc GPU cost is ~0.256 s vs CPU ~0.313 s (~18% faster per
document); the init is what single-shot burns. The GPU wins in batch
pipelines, not one-shot invocations.
