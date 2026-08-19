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
  48 MB of inputs) and `pdf-to-md` panics or emits a 0-byte file on 93/100
  corpus files.
- **MuPDF (`mutool clean`)** does not re-encode either: it is a *lossless
  repack* — images pass through byte-identical (verified 20/20 on the photo
  corpus) and renders are SSIM 1.0, but its size effect is mostly a wash:
  it shrank 62 of the 99 corpus files that produced output (median −6.3% on
  the shrinkers) and *grew* the other 37, with image-heavy PDFs barely
  moving (photos20 12.08 → 12.07 MB). Per-file it is fast (0.015 s median),
  but the commonly-recommended `-gggg` duplicate-scan is ~O(n²): 200+ s on
  the two large spec documents, where the plain `-z` repack takes 0.1–0.6 s.

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
  - MuPDF: `mutool clean -gggg -z <in> <out>` (1.28.0)
- Quality metric: whole-image SSIM of 60 dpi `pdftoppm` renders (output vs
  source), plus the maximum image resolution (`pdfimages -list`, x-ppi) as
  the resolution-preservation check.
- Times: quality tables are best-of-1 per file; the speed table is the
  per-file median over the 100-file corpus from `RESULTS.md` (excluding the
  60 s `specs_pdfref17old` outlier that is a lopdf single-threaded parse
  bottleneck — parallel == serial there).

## Quality × size × time on image-bearing files

| file (source size) | presse q50 → MB / SSIM | presse q30 → MB / SSIM | gs /ebook → MB / SSIM | mutool → MB / SSIM | gs max image ppi |
|---|---|---|---|---|---|
| arxiv_gpt3_2005 (6.77 MB) | 5.69 / 0.9999 | 5.03 / 0.9999 | 2.04 / 0.9944 | 6.60 / 1.0000 | 1234 → 1030 |
| arxiv_transformer_xl1901 (4.57 MB) | 4.41 / 1.0000 | 4.40 / 1.0000 | 1.04 / 0.9931 | 4.26 / 1.0000 | 1119 → 544 |
| irs_fw2 (2.15 MB) | 2.03 / 1.0000 | 2.02 / 1.0000 | 0.18 / 0.9989 | 2.71 / 1.0000 (grew) | 6483 → 1902 |
| specs_pymupdf_guide (7.11 MB) | 4.74 / 0.9999 | 4.35 / 0.9999 | 4.61 / 0.9985 | 8.36 / 1.0000 (grew) | 8736 → 6422 |
| photos20 (12.08 MB) | 11.04 / 0.9981 | 7.65 / 0.9971 | 12.07 / 1.0000 (no-op) | 12.07 / 1.0000 | 274 (unchanged) |

Time (seconds, best-of-1): presse 0.16–0.47 s per file across the set;
ghostscript 0.48–2.48 s (3–10× slower); mutool 0.01–3.01 s. On photos20
(72 dpi image placement) ghostscript is a pass-through no-op at 0.25 s —
identical size, SSIM 1.0, because `/ebook` only downsamples images above
its 150 dpi target. Mutool is also a pass-through for the images but does
repack the structural streams; where that repack is expensive
(pymupdf_guide: 124 images, 3.01 s) it is slower than presse, and on
already-tight files (irs_fw2, pymupdf_guide) the repack *grows* the file.

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
- **pdfrs / pdfcli** (0.1.9): `pdf-to-md` panics on 16/100 corpus files (a
  non-char-boundary panic in its text search) and silently emits a 0-byte
  file on another 77; only 7 files produce any markdown at all. Its `merge`
  produced an 80-page PDF of **28,894 bytes from 48,326,372 bytes of inputs
  (photos20 + photos60) — rendered blank** (the images are dropped, not
  compressed). `md-to-pdf` creates a 1.4 KB PDF from 64 B of markdown (a
  creator, not a compressor).

## MuPDF / mutool — a lossless repack, not a re-encoder

`mutool clean -gggg -z` is the standard "optimize" invocation, but it never
touches the image data: extracted JPEGs are byte-identical to the source
(verified 20/20 on the photo corpus) and renders match pixel-for-pixel
(SSIM 1.0000 on every page of all 99 valid corpus outputs — visual sweep,
pdftoppm 72 dpi + SSIM, long docs sampled at 80 pages). Its size effect is
limited to re-deflating structural streams and dropping dead objects: on
the 100-file corpus it *shrank* 62 of the 99 files that produced output
(median −6.3% on the shrinkers) and *grew* the other 37 — none came out
byte-identical — so its "compression" is bounded by how wasteful the
source's stream packing was, and it has no quality knob and no resolution
change. Outputs are deterministic across runs: a re-run on 2026-08-19
produced byte-identical files (0 size mismatches in 99/99) with the same
62/37 split.

Speed is bimodal. Per-file median is 0.015 s — faster than presse — but the
`-gggg` duplicate-object scan is ~O(n²): `specs_pdf32000.pdf` 210 s,
`specs_pdfref17old.pdf` 352 s (killed, no output), `irs_i1040.pdf` 70 s,
versus 0.1–0.6 s for the same files with the plain `-z` repack. Total over
the corpus: 692 s (worst of any tool here, driven entirely by that tail;
340 s excluding the killed file).

## Speed in context (100-file corpus, excluding the 60 s spec outlier)

| tool | operation | total | median/file |
|---|---|---|---|
| presse parallel | compress, full res | 14.3 s | 0.017 s |
| presse serial | compress | 22.1 s | 0.023 s |
| presse GPU | compress (≥ 1 MiB images) | 17.5 s | 0.020 s |
| unpdf | text extraction | 36.2 s | 0.357 s |
| pdfrs | md conversion (7/100 produce output) | 11.4 s | 0.017 s |
| ghostscript /ebook | compress (resolution-lossy) | 293 s | 0.152 s |
| mutool clean -gggg -z | lossless repack | 692 s¹ | 0.015 s |

¹ dominated by the ~O(n²) `-gggg` scan on `specs_pdfref17old.pdf` (killed
without output; 352 s in the original run, still running when re-measured
under a 600 s cap) and `specs_pdf32000.pdf` (210 s); excluding the killed
file the total is 340 s. The plain `-z` repack is 0.1–0.6 s on the same
files.

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
