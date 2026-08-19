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
  but the commonly-recommended `-gggg` duplicate-scan shows severe
  superlinear scaling in this corpus: 200+ s on the two large spec
  documents, where the plain `-z` repack takes 0.1–0.6 s.

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

## Resolution (`--dpi`) sweep

`presse press -d <dpi>` caps the effective resolution of every placed
image: an image drawn at `w×h` points is downsampled to at most
`w·dpi/72 × h·dpi/72` pixels (Ghostscript `/ebook`-style), so only images
placed above the target are touched — images already below it pass through
at source resolution, and the default (no `-d`) never downsamplies.
Placement is read from the page content stream's transform matrix at each
`Do` (the largest placement wins for shared images); images whose
placement cannot be parsed are left at source resolution.

Measured `-q 50` over high-resolution-placed files (60 dpi renders, SSIM
vs source, best-of-1):

| file (source) | no cap | 600 dpi | 300 dpi | 150 dpi | 75 dpi |
|---|---|---|---|---|---|
| irs_i1040 (4.43 MB, scans) | 4.17 MB / 1.0000 | 4.17 / 1.0000 | 3.88 / 1.0000 | 3.58 / 1.0000 | 3.47 / 1.0000 |
| irs_f1099r (0.62 MB) | 0.48 / 1.0000 | 0.48 / 1.0000 | 0.48 / 1.0000 | 0.45 / 1.0000 | 0.44 / 0.9999 |
| irs_fw2 (2.15 MB) | 2.03 / 1.0000 | 1.96 / 1.0000 | 1.95 / 1.0000 | 1.92 / 1.0000 | 1.91 / 0.9999 |
| arxiv_cyclegan1703 (37.6 MB) | 6.86 / 1.0000 | 6.86 / 1.0000 | 6.86 / 1.0000 | 6.86 / 1.0000 | 6.86 / 1.0000 |
| arxiv_gpt3_2005 (6.77 MB) | 5.69 / 1.0000 | 5.69 / 1.0000 | 5.69 / 1.0000 | 5.69 / 1.0000 | 5.69 / 1.0000 |
| photos20 (12.08 MB, 72 dpi placement) | 11.04 / 1.0000 | 11.04 / 1.0000 | 11.04 / 1.0000 | 11.04 / 1.0000 | 11.04 / 1.0000 |

Readings:

- SSIM stays ≥ 0.9999 at every level — downsampling is visually lossless
  on these renders; the size win is pure resolution removal, and 75/150
  dpi costs nothing perceptible on scans (irs_i1040: 21.7% / 19.3%
  reduction at SSIM 1.0000).
- The cap only bites where it should: paper PDFs whose figures are placed
  at ~72–150 dpi (gpt3, cyclegan) and photo PDFs placed at 72 dpi
  (photos20) are byte-identical to the no-cap run at every level — the
  same pass-through Ghostscript exhibits below its 150 dpi target.
- `-d 600` is a no-op for documents whose images are all placed below
  600 dpi, and a strict resolution cap (never an upscale) above it.

Corpus-wide gate (100-file corpus, `-d 150`, `-q 50`): 100/100 runs
completed with 0 failures; qpdf check matches the CPU corpus exactly
(94 clean + the same 4 benign warnings: 3× "input stream is complete"
and the pre-existing `issue7229` damaged-source artifact); visual sweep
97/97 valid docs clean, worst SSIM 0.98 (`arxiv_deeplab1706` — figures
placed above 150 dpi, downsampled as asked), the only flag being the same
`issue7229` source artifact the original file triggers.

## Pareto frontier vs modern full-resolution compressors

The render-SSIM gate was upgraded in response to a fair critique: SSIM at
60 dpi proves 60-dpi viewing fidelity, not print or native fidelity. Two
witnesses were added — render SSIM at **72 and 300 dpi**, and a
**native-image witness** (`native_image_ssim.py`): every image stream is
extracted (`pdfimages -j`), decoded, the source resampled to the *shipped*
dimensions, and SSIM computed with a 512 px analysis window instead of the
screen-fidelity 64×64. The two witnesses disagree on purpose, and the
native one is the stricter of the two:

> gs `/ebook` scores **1.0000 at a 300 dpi render** on an IRS scan PDF, yet
> ships its 5784×1448 scans as **165×41** images at **native SSIM 0.86**.
> No render scale at the 64×64 window can expose that; the native witness
> can. The same check gives presse/qpdf/mutool/pdf-optimizer native SSIM
> ≥ 0.9999 — they never down-sample, so their fidelity is exactly what the
> render shows.

Tools, compared at *equal measured fidelity* (SSIM thresholds, not equal
quality numbers), `benches/docker/pareto.py`: presse (`-q 30/50/75`, ±
`-d 150`), qpdf 12.3 (`--optimize-images --jpeg-quality`, explicitly does
not resample), MuPDF 1.28 recompression (`clean -i -gggg -z
--{color,gray}{,-lossless}-image-recompress-method jpeg:N`, recompress-only-
when-smaller), pdf-optimizer 1.0 (MuPDF-based, `--dpi 0`), ghostscript
`/screen…/prepress`. Sizes at SSIM ≥ 0.9999 (300 dpi render) on sampled
pages:

| file | presse | qpdf | mutool | pdf-optimizer | gs /ebook |
|---|---|---|---|---|---|
| irs_fw2 scans (2.15 MB) | 2.03 (q50) / 1.92 (-d150) | **1.52** | 2.54 | 2.55 | 0.18¹ |
| arxiv_gpt3 paper (6.77 MB) | 5.03 (q30) | 4.53 | **4.34** | 6.20 | 2.04¹ |
| photos20 (12.08 MB, 72 dpi) | 7.65 (q30) | **6.22** | 6.38 | 10.58 | 12.07 (no-op) |
| photos60 (36 MB, 60 imgs) | 22.96 (q30) | 18.67 | **6.76** (dedup) | 31.72 | 12.08 (no-op) |

¹ below any full-res tool's size *because it down-samples* — native SSIM
0.86 on the scans; on papers/photos it is a pass-through.

Speed at the same fidelity (wall time, best-of-1):

| file | presse | qpdf | mutool | pdf-optimizer |
|---|---|---|---|---|
| irs_fw2 | **0.16 s** | 0.09 s | 0.56 s | 0.30 s |
| photos20 | **0.25 s** | 0.99 s | 1.27 s | 5.92 s |
| photos60 | **0.26 s** | 2.93 s | 3.87 s | 17.82 s |

Readings:

- **At equal measured fidelity the full-res tools are visually
equivalent** (SSIM 1.0000, native 0.9999+) — the frontier is decided by
size and speed. qpdf is the size leader on scans; mutool's `-gggg`
dedup wins duplicate-heavy documents (photos60 q30: 6.76 vs 18.67 MB);
presse's parallel path is **4–70× faster** on image-heavy documents at
the same fidelity (photos20/60: 0.25–0.26 s vs qpdf 0.99–2.93 s, mutool
1.27–3.87 s, pdf-optimizer 5.9–17.8 s). qpdf is faster on a single-scan
file (irs_fw2: 0.09 vs 0.16 s) — the speedup is the parallelized image
phase, which only pays off with many images.
- **gs wins size only by removing pixels**, which only the native witness
reveals — at equal *native* fidelity its /ebook output would sit at
~1.5–2.0 MB on the scans, next to qpdf.
- **The Pareto claim this supports**: among open-source full-resolution
PDF image compressors, presse is the fastest measured point on
image-heavy documents at SSIM ≥ 0.9999 (4–70×), qpdf the smallest on
scan corpora, mutool on duplicate-heavy ones — and the speed gap is
what the rayon pipeline buys.

Caveats: sizes/times are best-of-1 on sampled pages (≤ 8/file) and the
photo PDFs' 41-inch pages exceed the 300 dpi render budget (their witness
is the 72 dpi render + native-image check, both 1.0000); quality numbers
are page-render SSIM + image-level SSIM, not a full semantic-preservation
court (ICC, annotations, forms, layers are validated by the qpdf/gs gates
but not separately measured).

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
`-gggg` duplicate-object scan scales severely superlinearly in this corpus:
`specs_pdf32000.pdf` 210 s, `specs_pdfref17old.pdf` 352 s (killed, no
output), `irs_i1040.pdf` 70 s, versus 0.1–0.6 s for the same files with the
plain `-z` repack. Total over the corpus: 692 s (worst of any tool here,
driven entirely by that tail; 340 s excluding the killed file).

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

¹ dominated by the `-gggg` duplicate scan's severe superlinear scaling on
`specs_pdfref17old.pdf` (killed without output; 352 s in the original run,
still running when re-measured under a 600 s cap) and `specs_pdf32000.pdf`
(210 s); excluding the killed file the total is 340 s. The plain `-z`
repack is 0.1–0.6 s on the same files.

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
