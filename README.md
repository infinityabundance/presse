# presse -- PDF manipulation command-line tool
<p align="center">
    <img src="./logo.webp" alt="Presse's logo" width=150 height=150/>
</p>

![demo](demo/demo.gif)

<p align="center">
    <img src="https://img.shields.io/crates/v/presse.svg?style=for-the-badge" alt="Crates.io">
    <a href="https://github.com/SimonBure/presse"><img src="https://img.shields.io/github/stars/SimonBure/presse?style=for-the-badge&label=Stars%20&logo=github&logoColor=white" alt="Presse stars" /></a>
</p>

A fast command-line tool for PDF manipulation written in Rust.

**Compress and merge** PDF files naturally and easily with this ready-to-use command line tool.
**Convert images** of any format into ready-to-use pdfs.

**~7× faster** than ghostscript (median, per-document on a 100-PDF corpus) and **~2× faster than single-threaded** rayon. Benchmarked in [`benches/docker/RESULTS.md`](benches/docker/RESULTS.md) — the headline figure is a median; best case is much higher, mean is lower.

## Features

- **Image recompression** — re-encodes images at a target quality, skipping CMYK images
- **Engine optimizations** — rayon-parallel re-encoding, mimalloc allocator, zlib-rs deflate backend, memory-mapped reads, deduplicated identical image streams
- **Structural compression** — object stream packing, xref stream compression
- **Batch processing** — compress multiple files in one command via shell wildcards
- **Smart output paths** — sensible defaults, explicit naming, or output to a directory
- **PDF merging** — combine multiple documents into one, with optional compression
- **Image conversion** — transform any image (.png, .jpg) into a .pdf
- **Merge docs and images** in a single command, with smart format detection

## Installation

### Shell installer (macOS/Linux)
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/SimonBure/presse/releases/latest/download/presse-installer.sh | sh
```

### PowerShell (Windows)
```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/SimonBure/presse/releases/latest/download/presse-installer.ps1 | iex"
```

### MSI (Windows)
Download the `.msi` from the [latest release](https://github.com/SimonBure/presse/releases/latest).

### Cargo
```bash
cargo install presse
```

## Benchmark

Measured over 100 public real-world PDFs (≈300 MB, 5,268 pages), comparing
`presse press --quality 50` against Ghostscript `/ebook`. Full methodology,
per-document data, and correctness gates in
[`benches/docker/RESULTS.md`](benches/docker/RESULTS.md).

| | presse (rayon) | presse (serial) | ghostscript `/ebook` |
|---|---|---|---|
| Total wall time (100 PDFs) | **81 s** | 123 s | 319 s |
| Mean per document | **0.81 s** | 1.23 s | 3.22 s |
| Mean size reduction | 19.5 % | 19.5 % | 60.5 % |

- **Parallel vs single-threaded**: 2.2× mean, 1.5× median. Best case ~6× on
  an image-heavy PDF (24 images, 16 cores) — the headline speedup of the
  rayon pipeline.
- **Parallel vs ghostscript**: ~7× median per document, ~4× total wall time.
- **Size**: presse reduces output without touching image resolution;
  ghostscript's `/ebook` downsamples images, so it shrinks files more but
  changes their resolution. Both figures are means over the corpus.

A reproducible containerized benchmark harness lives in [`benches/docker/`](benches/docker/):

```bash
docker build -t presse-bench -f benches/docker/Dockerfile.bench .
docker run --rm presse-bench
```

The container builds with `RUSTFLAGS="-C target-cpu=native"` (AVX2/FMA auto-vectorization) and reports wall time, throughput, and peak RSS for a generated corpus (text-heavy, image-heavy, scanned), comparing the rayon pipeline against a `RAYON_NUM_THREADS=1` baseline.

## Usage

### Compress — `presse press`

```bash
# Single file — outputs document_compressed.pdf alongside the original
presse press document.pdf

# Custom output name
presse press document.pdf -o small.pdf

# Output to a directory
presse press document.pdf -o compressed/

# Batch — multiple files into a directory
presse press *.pdf -o compressed/

# Set JPEG quality (0–100, default 80)
presse press document.pdf --quality 60

# Cap image resolution to 150 dpi (75 screen, 150 ebook, 300 printer, 600 prepress)
presse press document.pdf -d 150

# Show size comparison after each file
presse press document.pdf --verbose

# Combine everything in a merged.pdf
presse merge *.pdf

# Convert images into pdfs
presse convert img1.png img2.jpg img3.jpeg

# Convert all png images, compressed them and merged them into a single pdf 
presse convert *.png -m -q 50

# Merge image and pdf together into a single pdf
presse merge *.png *.pdf
```

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output` | `<input>_compressed.pdf` | Output file or directory |
| `-q, --quality` | `80` | Image recompression quality (0–100) |
| `-d, --dpi` | source resolution | Cap placed image resolution to DPI pixels/inch (75 screen, 150 ebook, 300 printer, 600 prepress); omitted = keep source resolution |
| `-s, --ssim` | `1.0` | Target output fidelity as measured SSIM (calibrated on grainy scans — the worst case for JPEG — so smoother content exceeds the target); lower = smaller and faster. `1.0` = use `-q` as given |
| `--palette` | `false` | Also build an `/Indexed` palette candidate for eligible flat-color images (figures, charts, scans) and keep the smallest of original / JPEG / indexed; exact palettes are lossless, lossy ones are gated on 0.9999 native SSIM |
| `-a, --acceleration` | `cpu` | Image transcoding backend: `cpu`, `auto`, `cuda`, or `rocm` (GPU backends require a feature build — see [GPU acceleration](#gpu-acceleration-experimental)) |
| `-v, --verbose` | `false` | Print size comparison after each file |

### Resolution capping (`--dpi`)

`-d <dpi>` downsamples images to at most `w·dpi/72 × h·dpi/72` pixels,
where `w × h` is the image's placed size on the page in points — the same
placement-aware rule Ghostscript applies with `-dPDFSETTINGS`. The cap is
strict: it **never up-samples**, and images whose on-page placement cannot
be determined (or that are already below the cap) keep their source
resolution. Only `press` takes the flag today.

```bash
presse press big.pdf -o small.pdf -d 150   # ~ebook: 150 dpi cap
presse press scans.pdf -d 75               # ~screen: 75 dpi cap
presse press deck.pdf -s 0.86              # ~q9: fidelity-targeted, 60% smaller on photos
presse press figures.pdf --palette         # also try indexed-color palettes
```

### Duplicate-image collapsing and palette quantization

Two structural optimizations are always on:

- **Coalescing** — after re-encoding, image streams that are semantically
  identical (same dictionary and same payload) collapse onto one object,
  and every reference is rewritten to it. Documents that embed the same
  photo, logo, watermark or figure many times (60 photos that are really
  20 unique images × 3 copies) shrink to the unique content once — e.g.
  22.96 → 6.46 MB at `-q 30` on the photos60 corpus file, below MuPDF's
  dedup result, with identical rendering.
- **Flate-wrapped JPEG** — DCT streams are normally incompressible, but
  padded/progressive JPEGs occasionally shrink under zlib; the
  `[FlateDecode, DCTDecode]` chain is used only when the full flate result
  is smaller.

`--palette` (off by default) additionally tries an `/Indexed` color-space
candidate for eligible flat-color images: one index byte per pixel plus a
≤256-entry palette, Flate-compressed — the representation that beats JPEG
on figures, charts, diagrams and screenshots. Images with ≤256 unique
colors convert losslessly; larger rasters go through a deterministic
median-cut quantizer and are accepted only above a 0.9999 native-image SSIM
gate, so a lossy palette can never visibly degrade a figure. Photos
effectively never qualify. The smallest of original / JPEG / indexed wins
per image. Only `press` takes the flag today.

### Merge — `presse merge`

```bash
# Merge two or more files — outputs merged.pdf in the current directory
presse merge a.pdf b.pdf c.pdf

# Custom output name
presse merge a.pdf b.pdf -o result.pdf

# Output to a directory
presse merge a.pdf b.pdf -o output/

# Also compress images while merging
presse merge a.pdf b.pdf --compress
```

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output` | `merged.pdf` | Output file or directory |
| `-c, --compress` | `false` | Compress images in the merged document |

## GPU acceleration (experimental)

`presse press` can offload JPEG re-encoding to a GPU with
`--acceleration cuda` (NVIDIA nvJPEG) or `--acceleration rocm` (AMD
rocJPEG). This is experimental, opt-in work — neither backend is linked
into the release binaries, neither runs in CI, and the CPU backend is the
default and always available. On a default build, `--acceleration cuda`
or `rocm` fails with an explicit "requires a build with the … feature"
error; build with the feature enabled to use them:

```bash
# NVIDIA
cargo install presse --features cuda --locked

# AMD
cargo install presse --features rocm --locked
```

`--locked` matters: the `baracuda` requirement is a caret range over
pre-releases, so without it Cargo may resolve a version the code has not
been tested against.

Runtime needs:

- **cuda** — an NVIDIA driver and the `nvjpeg` shared library. No CUDA
  toolkit is needed to *build*: the vendor library is loaded at runtime.
  Validated on an RTX 4080 SUPER with CUDA 13.3.
- **rocm** — compile-tested only; requires a ROCm installation with
  `rocjpeg` at runtime.

If the driver or library is missing at runtime, presse warns and falls
back to the CPU encoder per stream — a broken GPU can never drop or
corrupt a stream. Measured behavior (threshold routing, speed vs size
tradeoffs) is documented in [`benches/docker/RESULTS.md`](benches/docker/RESULTS.md).

## Limitations

- CMYK images are not compressed (not currently handled by `image` crate)

## Dependencies

- [lopdf](https://github.com/niclasberg/lopdf) — PDF parsing and manipulation
- [clap](https://github.com/clap-rs/clap) — CLI argument parsing
- [indicatif](https://github.com/console-rs/indicatif) — Progress bars
- [image](https://github.com/image-rs/image) — JPEG decoding and encoding
- [rayon](https://github.com/rayon-rs/rayon) — parallel image re-encoding

## Contributions
We are happy to welcome contributions! Pull requests are welcome.

## License
[GPL-3.0](LICENSE)


Made with ❤️ in Paris.