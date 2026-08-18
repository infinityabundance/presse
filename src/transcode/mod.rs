//! Pluggable image transcoding backends.
//!
//! The default [`CpuTranscoder`] is the rayon + 32 KiB chunked SIMD engine
//! and is the only backend linked into default builds — zero GPU
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
//! SUPER with CUDA 13.3 (decode + encode, including the runtime fallback
//! path); the `rocm` backend is compile-tested only. Neither backend runs in
//! CI — validate on real hardware before relying on GPU acceleration in
//! production.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "cuda")]
pub mod cuda;
#[cfg(feature = "rocm")]
pub mod rocm;

/// Minimum encoder-input size (bytes) before a GPU backend is consulted.
/// Below this, PCIe transfer overhead exceeds the decode/encode time saved,
/// so small streams always stay on the CPU path.
// Used only by the `cuda`/`rocm` feature-gated backends.
#[allow(dead_code)]
pub const GPU_MIN_STREAM_BYTES: usize = 128 * 1024;

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
#[derive(Debug)]
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
            0xD8 | 0xD0..=0xD7 => i += 2, // SOI / RSTn
            0xD9 | 0xDA => return None,   // EOI / SOS: no frame seen
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

/// Key for the transcode dedup cache: target quality + a path tag (1 = luma,
/// 2 = jpeg, 3 = rgb) + the exact encoder-input bytes.
pub(crate) fn encode_key(quality: u8, tag: u8, bytes: &[u8]) -> u64 {
    let mut h = ahash::AHasher::default();
    h.write_u8(quality);
    h.write_u8(tag);
    bytes.hash(&mut h);
    h.finish()
}

/// Shared handle for caching transcode results across duplicate streams.
/// Empty buffers and errors are cached too, so every duplicate image object
/// reproduces the same decision without re-running the backend.
pub(crate) struct TranscodeCache {
    map: std::sync::Mutex<CacheMap>,
}

type CacheMap = std::collections::HashMap<u64, Arc<Result<Vec<u8>, TranscodeError>>>;

impl TranscodeCache {
    pub(crate) fn new() -> Self {
        Self {
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) fn get(
        &self,
        key: u64,
        produce: impl FnOnce() -> Result<Vec<u8>, TranscodeError>,
    ) -> Arc<Result<Vec<u8>, TranscodeError>> {
        if let Some(hit) = self.map.lock().unwrap().get(&key) {
            return Arc::clone(hit);
        }
        let produced = Arc::new(produce());
        self.map
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::clone(&produced));
        produced
    }
}

#[cfg(test)]
mod tests {
    use super::jpeg_components;

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
}
