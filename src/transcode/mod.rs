//! Pluggable image transcoding backends.
//!
//! The default [`CpuTranscoder`] is a rayon-parallel JPEG re-encoder — the
//! only explicit fixed-size block loop is the RGBA→RGB normalization pass;
//! there is no hand-written SIMD, the codecs are `image`-crate based and may
//! use SIMD internally, and `-C target-cpu=native` lets LLVM auto-vectorize
//! the rest. It is the only backend linked into default builds — zero GPU
//! dependencies and zero dynamic dispatch on the default path (callers are
//! generic over [`ImageTranscoder`], so the CPU backend monomorphizes).
//!
//! GPU backends are opt-in Cargo features (`cuda`, `rocm`) selected with
//! `--acceleration`. They load the vendor libraries at *runtime* (no
//! link-time driver dependency), and every failure — missing library,
//! missing driver, context/VRAM errors, per-stream decode/encode errors —
//! degrades to the CPU backend, so a broken or absent GPU can never drop or
//! corrupt a stream.
//!
//! **Hardware note:** the `cuda` backend was exercised on an NVIDIA RTX 4080
//! SUPER with CUDA 13.3 (batched decode to device memory with the hardware
//! decoder, per-image encode from device memory, runtime fallback paths);
//! the `rocm` backend is compile-tested only. Neither backend runs in CI —
//! validate on real hardware before relying on GPU acceleration in
//! production.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

#[cfg(feature = "cuda")]
pub mod cuda;
#[cfg(feature = "rocm")]
pub mod rocm;

/// Minimum encoder-input size (bytes) before a GPU backend is consulted.
/// Below this, streams always stay on the CPU path.
///
/// Measured crossover (RTX 4080 SUPER, 16 cores, `-C target-cpu=native`,
/// q50): routing small/medium images through the single GPU consumer thread
/// serializes work that the 16 rayon workers parallelize better, so the GPU
/// pays off only on the largest images of a document — where offloading
/// them frees the CPU to keep processing the rest concurrently. Sweeping
/// the threshold over mixed / small-heavy / photo documents, 1 MiB was the
/// best value: 26–60% faster than the previous 128 KiB default and faster
/// than the pure-CPU path on the mixed corpus. Documents whose images are
/// uniformly huge (all >1 MiB) remain a wash — the GPU ties the CPU
/// per-image there, so no threshold helps.
// Used only by the `cuda`/`rocm` feature-gated backends.
#[allow(dead_code)]
pub const GPU_MIN_STREAM_BYTES: usize = 1024 * 1024;

/// Decoded pixels ready for re-encoding.
#[derive(Debug, Clone, Copy)]
pub enum ImageRef<'a> {
    /// Single-channel (grayscale) pixels.
    Luma8 {
        width: u32,
        height: u32,
        bytes: &'a [u8],
    },
    /// Interleaved RGB pixels.
    Rgb8 {
        width: u32,
        height: u32,
        bytes: &'a [u8],
    },
}

/// The input to one transcode call.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// JPEG/JPX stream bytes (a `/DCTDecode` image stream).
    Jpeg(&'a [u8]),
    /// Decoded pixels of a raw stream, after any channel normalization.
    Pixels(ImageRef<'a>),
}

/// Transcoding failure. All variants are safe to surface to the user; the
/// caller decides whether to skip the stream or retry on another backend.
// `Encode`/`Unavailable`/`Gpu` are produced only by the feature-gated backends.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum TranscodeError {
    /// The input could not be decoded.
    Decode(String),
    /// Encoding produced no usable output.
    Encode(String),
    /// The backend could not be initialized (missing library/driver/context).
    Unavailable(String),
    /// A backend operation failed at runtime.
    Gpu(String),
}

impl fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranscodeError::Decode(m) => write!(f, "decode failed: {m}"),
            TranscodeError::Encode(m) => write!(f, "encode failed: {m}"),
            TranscodeError::Unavailable(m) => write!(f, "backend unavailable: {m}"),
            TranscodeError::Gpu(m) => write!(f, "gpu error: {m}"),
        }
    }
}

impl std::error::Error for TranscodeError {}

/// Pluggable JPEG re-encoding backend.
pub trait ImageTranscoder: Send + Sync {
    /// Re-encode one image stream at the target quality, returning the new
    /// JPEG payload. Implementations must be deterministic for a given input.
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError>;
}

/// The default backend: CPU JPEG decode/encode with the `image` crate.
/// Grayscale payloads are encoded as single-component JPEGs (see
/// [`encode_jpeg`]), matching `/DeviceGray` streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct CpuTranscoder;

impl ImageTranscoder for CpuTranscoder {
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        let img = match input {
            Input::Jpeg(bytes) => image::load_from_memory(bytes)
                .map_err(|e| TranscodeError::Decode(format!("jpeg decode: {e}")))?,
            Input::Pixels(ImageRef::Luma8 {
                width,
                height,
                bytes,
            }) => image::GrayImage::from_raw(*width, *height, bytes.to_vec())
                .map(image::DynamicImage::ImageLuma8)
                .ok_or_else(|| TranscodeError::Decode("bad grayscale dimensions".into()))?,
            Input::Pixels(ImageRef::Rgb8 {
                width,
                height,
                bytes,
            }) => image::RgbImage::from_raw(*width, *height, bytes.to_vec())
                .map(image::DynamicImage::ImageRgb8)
                .ok_or_else(|| TranscodeError::Decode("bad RGB dimensions".into()))?,
        };
        let mut out = Vec::new();
        encode_jpeg(&mut out, &img, quality);
        Ok(out)
    }
}

/// Routes streams to a GPU backend when one is available and the stream is
/// large enough to beat PCIe transfer overhead; everything else (and every
/// GPU failure) goes to the CPU backend. A warning is emitted once per
/// process on the first GPU failure, and output is never dropped.
// Constructed only by the `cuda`/`rocm` feature-gated `resolve` paths.
#[allow(dead_code)]
#[derive(Debug)]
pub struct FallbackTranscoder<G> {
    cpu: CpuTranscoder,
    gpu: Option<G>,
    min_bytes: usize,
    warned: AtomicBool,
}

impl<G: ImageTranscoder> FallbackTranscoder<G> {
    /// `gpu: None` disables the GPU path entirely (e.g. init failed).
    #[allow(dead_code)] // called only by the feature-gated `resolve` paths
    pub fn new(gpu: Option<G>, min_bytes: usize) -> Self {
        Self {
            cpu: CpuTranscoder,
            gpu,
            min_bytes,
            warned: AtomicBool::new(false),
        }
    }
}

impl<G: ImageTranscoder> ImageTranscoder for FallbackTranscoder<G> {
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        let len = match input {
            Input::Jpeg(bytes) => bytes.len(),
            Input::Pixels(ImageRef::Luma8 { bytes, .. })
            | Input::Pixels(ImageRef::Rgb8 { bytes, .. }) => bytes.len(),
        };

        if let Some(gpu) = &self.gpu
            && len >= self.min_bytes
        {
            match gpu.transcode_image(input, quality) {
                Ok(buf) => return Ok(buf),
                Err(e) => {
                    if !self.warned.swap(true, Ordering::Relaxed) {
                        eprintln!(
                            "warning: GPU transcoding failed ({e}); falling back to CPU for this stream"
                        );
                    }
                }
            }
        }
        self.cpu.transcode_image(input, quality)
    }
}

/// The `--acceleration` CLI selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Acceleration {
    /// Use a GPU backend when one is available, CPU otherwise.
    Auto,
    /// Always use the CPU backend (default).
    #[default]
    Cpu,
    /// Use the NVIDIA nvJPEG backend (requires the `cuda` feature).
    Cuda,
    /// Use the AMD rocJPEG backend (requires the `rocm` feature).
    Rocm,
}

/// A concrete transcoder selected at startup.
#[derive(Debug)]
pub enum RuntimeTranscoder {
    Cpu(CpuTranscoder),
    #[cfg(feature = "cuda")]
    Cuda(FallbackTranscoder<cuda::CudaTranscoder>),
    #[cfg(feature = "rocm")]
    Rocm(FallbackTranscoder<rocm::RocmTranscoder>),
}

impl ImageTranscoder for RuntimeTranscoder {
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        match self {
            RuntimeTranscoder::Cpu(t) => t.transcode_image(input, quality),
            #[cfg(feature = "cuda")]
            RuntimeTranscoder::Cuda(t) => t.transcode_image(input, quality),
            #[cfg(feature = "rocm")]
            RuntimeTranscoder::Rocm(t) => t.transcode_image(input, quality),
        }
    }
}

/// Resolve an `--acceleration` selector to a concrete backend.
///
/// Requesting a backend that was not compiled in (`cuda`/`rocm` without the
/// matching Cargo feature) is an explicit error naming the missing flag.
/// Requesting a compiled-in backend whose library/driver is missing at
/// runtime warns and degrades to the CPU backend.
pub fn resolve(acceleration: Acceleration) -> Result<RuntimeTranscoder, String> {
    match acceleration {
        Acceleration::Cpu => Ok(RuntimeTranscoder::Cpu(CpuTranscoder)),
        Acceleration::Auto => {
            #[cfg(feature = "cuda")]
            if let Ok(gpu) = cuda::CudaTranscoder::new() {
                return Ok(RuntimeTranscoder::Cuda(FallbackTranscoder::new(
                    Some(gpu),
                    GPU_MIN_STREAM_BYTES,
                )));
            }
            #[cfg(feature = "rocm")]
            if let Ok(gpu) = rocm::RocmTranscoder::new() {
                return Ok(RuntimeTranscoder::Rocm(FallbackTranscoder::new(
                    Some(gpu),
                    GPU_MIN_STREAM_BYTES,
                )));
            }
            Ok(RuntimeTranscoder::Cpu(CpuTranscoder))
        }
        Acceleration::Cuda => {
            #[cfg(feature = "cuda")]
            {
                match cuda::CudaTranscoder::new() {
                    Ok(gpu) => Ok(RuntimeTranscoder::Cuda(FallbackTranscoder::new(
                        Some(gpu),
                        GPU_MIN_STREAM_BYTES,
                    ))),
                    Err(e) => {
                        eprintln!("warning: {e}; falling back to the CPU backend");
                        Ok(RuntimeTranscoder::Cpu(CpuTranscoder))
                    }
                }
            }
            #[cfg(not(feature = "cuda"))]
            Err(
                "--acceleration cuda requires a build with the `cuda` feature: \
                 run `cargo build --features cuda`"
                    .into(),
            )
        }
        Acceleration::Rocm => {
            #[cfg(feature = "rocm")]
            {
                match rocm::RocmTranscoder::new() {
                    Ok(gpu) => Ok(RuntimeTranscoder::Rocm(FallbackTranscoder::new(
                        Some(gpu),
                        GPU_MIN_STREAM_BYTES,
                    ))),
                    Err(e) => {
                        eprintln!("warning: {e}; falling back to the CPU backend");
                        Ok(RuntimeTranscoder::Cpu(CpuTranscoder))
                    }
                }
            }
            #[cfg(not(feature = "rocm"))]
            Err(
                "--acceleration rocm requires a build with the `rocm` feature: \
                 run `cargo build --features rocm`"
                    .into(),
            )
        }
    }
}

/// JPEG marker segment types that carry a frame header (SOF).
/// `0xC4` (DHT), `0xC8` (JPG) and `0xCC` (DAC) are excluded.
const SOF_MARKERS: &[u8] = &[
    0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
];

/// Number of components declared by the JPEG frame header, if the stream is
/// a decodable baseline/progressive JPEG.
///
/// GPU backends use this as a guard: nvJPEG/rocJPEG expose no single-channel
/// (grayscale) input format, so 1-component JPEGs must be rejected before
/// they reach the GPU — otherwise the decoded RGB output would be written
/// into a `/DeviceGray` stream and rendered as garbage by viewers.
// Used only by the `cuda`/`rocm` feature-gated backends.
#[allow(dead_code)]
pub(crate) fn jpeg_components(data: &[u8]) -> Option<u8> {
    let mut i = 2; // skip SOI
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        match marker {
            0xD0..=0xD8 => i += 2,      // RSTn / SOI
            0xD9 | 0xDA => return None, // EOI / SOS: no frame seen
            m if SOF_MARKERS.contains(&m) => {
                // precision(1) height(2) width(2) components(1)
                return Some(data[i + 9]);
            }
            _ => i += 2 + u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize,
        }
    }
    None
}

/// Encode `img` as JPEG at the given quality, preserving grayscale payloads.
///
/// `DynamicImage` always presents an RGB pixel type to the encoder, so a
/// Luma8 payload would be written as a 3-component JPEG — invalid inside a
/// `/DeviceGray` stream and rendered as washed-out garbage by viewers.
/// Encoding the concrete `GrayImage` keeps the JPEG at a single component.
pub(crate) fn encode_jpeg(out: &mut Vec<u8>, img: &image::DynamicImage, quality: u8) {
    use image::codecs::jpeg::JpegEncoder;
    let mut encoder = JpegEncoder::new_with_quality(std::io::Cursor::new(out), quality);
    match img {
        image::DynamicImage::ImageLuma8(gray) => {
            let _ = encoder.encode_image(gray);
        }
        other => {
            let _ = encoder.encode_image(other);
        }
    }
}

/// Semantic identity of one transcode request: everything that determines
/// the output JPEG, independent of which object happened to carry the
/// pixels.
///
/// The identity is the *encoder input*, not the source stream — two streams
/// that decode to identical pixels at the same size share one entry and one
/// encode (e.g. a `/FlateDecode` and an `/LZWDecode` twin, or a 4-byte
/// `DeviceRGB` stream normalized to 3 bytes). Width and height are part of
/// the key because two images can carry identical pixel bytes at different
/// dimensions (RGB 10×10 and RGB 20×5 are both 300 bytes) and must still
/// emit JPEGs with their own headers. Equality is decided on the exact
/// content bytes, never on a hash alone: the map may hash the key for
/// placement, but a hash collision can never substitute one image's JPEG for
/// another's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    /// Target quality.
    quality: u8,
    /// Input kind: 1 = luma pixels, 2 = JPEG bytes, 3 = RGB pixels. Pins
    /// the encoder-input pixel format together with the content bytes.
    kind: u8,
    /// Raster width of the encoder input.
    width: u32,
    /// Raster height of the encoder input.
    height: u32,
    /// Exact encoder-input bytes (content identity).
    content: Arc<[u8]>,
}

/// Build a [`CacheKey`] for one transcode request.
pub(crate) fn encode_key(quality: u8, kind: u8, width: u32, height: u32, bytes: &[u8]) -> CacheKey {
    CacheKey {
        quality,
        kind,
        width,
        height,
        content: Arc::from(bytes),
    }
}

/// Shared handle for caching transcode results across duplicate streams.
/// Empty buffers and errors are cached too, so every duplicate image object
/// reproduces the same decision without re-running the backend.
///
/// The map lock is held only for the find-or-insert of the (cheap) entry;
/// the encode itself runs outside it, and the per-entry [`OnceLock`] makes
/// the first producer's result visible to every worker that raced on the
/// same key — `get_or_init` blocks the losers until the winner finishes, so
/// a given key is computed exactly once even under full contention.
pub(crate) struct TranscodeCache {
    map: std::sync::Mutex<CacheMap>,
}

type CacheMap = std::collections::HashMap<CacheKey, Entry>;

/// One cached transcode result, shared by every worker that hit the same key.
type CachedResult = Arc<Result<Vec<u8>, TranscodeError>>;

struct Entry {
    /// `OnceLock` for once-only production; the outer `Arc` is the handle we
    /// can take out of the map lock so callers never hold it while encoding.
    value: Arc<OnceLock<CachedResult>>,
}

impl Entry {
    fn new() -> Self {
        Self {
            value: Arc::new(OnceLock::new()),
        }
    }
}

impl TranscodeCache {
    pub(crate) fn new() -> Self {
        Self {
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) fn get(
        &self,
        key: CacheKey,
        produce: impl FnOnce() -> Result<Vec<u8>, TranscodeError>,
    ) -> Arc<Result<Vec<u8>, TranscodeError>> {
        let value = {
            let mut guard = self.map.lock().unwrap();
            Arc::clone(&guard.entry(key).or_insert_with(Entry::new).value)
        };
        Arc::clone(value.get_or_init(|| Arc::new(produce())))
    }
}

#[cfg(test)]
mod tests {
    use super::jpeg_components;
    use super::{TranscodeCache, TranscodeError, encode_key};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a minimal JPEG with the given component count by emitting a
    /// scan-less frame header (SOF0) after SOI.
    fn fake_jpeg(components: u8) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8]; // SOI
        b.extend_from_slice(&[0xFF, 0xC0]); // SOF0
        b.extend_from_slice(&[0x00, 0x11]); // length 17
        b.push(8); // precision
        b.extend_from_slice(&[0x00, 0x40, 0x00, 0x40]); // 64x64
        b.push(components);
        for i in 0..components {
            b.extend_from_slice(&[i, 0x11, 0x00]); // id, sampling, quant table
        }
        b
    }

    #[test]
    fn jpeg_components_counts_channels() {
        assert_eq!(jpeg_components(&fake_jpeg(1)), Some(1));
        assert_eq!(jpeg_components(&fake_jpeg(3)), Some(3));
    }

    #[test]
    fn jpeg_components_skips_non_frame_segments() {
        // APP0 before SOF0 must be skipped, and non-JPEG input must yield None.
        let mut b = vec![0xFF, 0xD8];
        b.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]); // APP0, length 16
        b.extend_from_slice(&[
            0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
        ]); // 14-byte payload (APP0 length 16 includes its own 2 bytes)
        b.extend_from_slice(&fake_jpeg(3)[2..]);
        assert_eq!(jpeg_components(&b), Some(3));
        assert_eq!(jpeg_components(b"not a jpeg"), None);
        assert_eq!(jpeg_components(&[]), None);
    }

    #[test]
    fn cache_key_distinguishes_dimensions_with_identical_bytes() {
        // RGB 10×10 and RGB 20×5 both hold 300 identical bytes; their keys
        // must differ so a cached 10×10 JPEG is never handed to the 20×5
        // image (each would carry the wrong /Width /Height header).
        let bytes = vec![7u8; 300];
        let a = encode_key(50, 3, 10, 10, &bytes);
        let b = encode_key(50, 3, 20, 5, &bytes);
        assert_ne!(a, b);

        // and the cache keeps them as distinct entries
        let cache = TranscodeCache::new();
        let ra = cache.get(a, || Ok(vec![1]));
        let rb = cache.get(b, || Ok(vec![2]));
        assert_eq!(ra.as_ref().as_deref().unwrap(), &[1][..]);
        assert_eq!(rb.as_ref().as_deref().unwrap(), &[2][..]);
    }

    #[test]
    fn cache_key_distinguishes_quality_kind_and_content() {
        let bytes = vec![7u8; 12];
        let other = vec![8u8; 12];
        assert_ne!(
            encode_key(30, 3, 4, 3, &bytes),
            encode_key(50, 3, 4, 3, &bytes)
        );
        assert_ne!(
            encode_key(50, 1, 4, 3, &bytes),
            encode_key(50, 3, 4, 3, &bytes)
        );
        assert_ne!(
            encode_key(50, 3, 4, 3, &bytes),
            encode_key(50, 3, 4, 3, &other)
        );
    }

    #[test]
    fn cache_produces_once_for_repeated_keys() {
        let cache = TranscodeCache::new();
        let produced = AtomicUsize::new(0);
        let key = encode_key(50, 3, 4, 3, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        for _ in 0..4 {
            let r = cache.get(key.clone(), || {
                produced.fetch_add(1, Ordering::SeqCst);
                Ok(b"jpeg".to_vec())
            });
            assert_eq!(r.as_ref().as_deref().unwrap(), b"jpeg");
        }
        assert_eq!(produced.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_converges_on_one_computation_under_contention() {
        // Workers that race on the same key must all converge on the single
        // encode the winner performs, not each run their own.
        let cache = TranscodeCache::new();
        let produced = Arc::new(AtomicUsize::new(0));
        let key = encode_key(50, 2, 64, 64, &[9; 128]);
        std::thread::scope(|s| {
            for _ in 0..8 {
                let cache = &cache;
                let key = key.clone();
                let produced = Arc::clone(&produced);
                s.spawn(move || {
                    let r = cache.get(key, || {
                        produced.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![4; 16])
                    });
                    assert_eq!(r.as_ref().as_deref().unwrap(), &[4; 16][..]);
                });
            }
        });
        assert_eq!(produced.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_caches_errors_and_empty_outputs() {
        let cache = TranscodeCache::new();
        let produced = AtomicUsize::new(0);
        let err_key = encode_key(50, 3, 2, 2, &[0; 12]);
        let empty_key = encode_key(50, 3, 2, 2, &[1; 12]);

        // errors are cached: the second lookup does not re-run the backend
        let r1 = cache.get(err_key.clone(), || {
            produced.fetch_add(1, Ordering::SeqCst);
            Err(TranscodeError::Decode("boom".into()))
        });
        assert!(r1.is_err());
        let r2 = cache.get(err_key, || {
            produced.fetch_add(1, Ordering::SeqCst);
            Err(TranscodeError::Decode("boom".into()))
        });
        assert!(r2.is_err());
        assert_eq!(produced.load(Ordering::SeqCst), 1);

        // empty buffers are cached too, so duplicates make the same decision
        let e1 = cache.get(empty_key.clone(), || {
            produced.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        });
        assert!(e1.as_ref().as_deref().unwrap().is_empty());
        let e2 = cache.get(empty_key, || {
            produced.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        });
        assert!(e2.as_ref().as_deref().unwrap().is_empty());
        assert_eq!(produced.load(Ordering::SeqCst), 2);
    }
}
