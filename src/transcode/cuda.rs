//! NVIDIA nvJPEG backend (`--acceleration cuda`, feature = "cuda").
//!
//! Uses [`baracuda-nvjpeg-sys`]'s dynamic loader, which dlopens
//! `libnvjpeg.so`/`nvjpeg64.dll` at runtime — no CUDA toolkit needed to
//! build, no link-time driver dependency. Any failure (library missing,
//! driver missing, context/VRAM errors, per-stream errors) surfaces as
//! [`TranscodeError`] and the caller's [`FallbackTranscoder`] degrades to
//! the CPU backend.
//!
//! Pipeline: every [`Inner`] owns a dedicated CUDA stream. The decode and
//! encode calls are enqueued on that stream (async), so kernels and the
//! host↔device copies of one image overlap with other handles' streams —
//! the pool lets each rayon worker keep the GPU busy. JPEGs are decoded to
//! planar YUV (1.5 bytes/pixel, no RGB round-trip) and re-encoded from YUV
//! at the source chroma subsampling; formats nvJPEG cannot do in YUV (odd
//! subsamplings, raw RGB streams) fall back to interleaved RGB. Decode
//! output and the encoded bitstream use pinned host memory
//! ([`cudaHostAlloc`]) when available so the copies are DMA-direct instead
//! of staged through pageable bounce buffers.
//!
//! nvJPEG exposes no single-channel input format, so `/DeviceGray` streams
//! and 1-component JPEGs are routed back to the CPU backend. nvJPEG states
//! are not thread-safe, so each [`Inner`] is used by one thread at a time
//! via the handle pool in [`CudaTranscoder`].

use baracuda_cuda_sys::runtime::{self as cudart, cudaError_t, cudaStream_t};
use baracuda_nvjpeg_sys as nvjpeg;

use crate::transcode::{ImageRef, ImageTranscoder, Input, TranscodeError, jpeg_components};
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::Mutex;

/// `nvjpegInputFormat_t` for the interleaved-RGB fallback encode.
const NVJPEG_INPUT_RGBI: c_int = 5; // interleaved RGB

/// `nvjpegChromaSubsampling_t` values returned by `nvjpegGetImageInfo`.
const CSS_444: c_int = 0;
const CSS_422: c_int = 1;
const CSS_420: c_int = 2;

fn check(status: nvjpeg::nvjpegStatus_t) -> Result<(), TranscodeError> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(TranscodeError::Gpu(format!("nvjpeg status {}", status.0)))
    }
}

fn check_cuda(status: cudaError_t) -> Result<(), TranscodeError> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(TranscodeError::Gpu(format!("cuda error {}", status.0)))
    }
}

/// Host memory that is pinned (`cudaHostAlloc`) for DMA-direct copies when
/// possible, falling back to the heap when pinned allocation is unavailable.
/// Pinned memory is what makes the async pipeline fast: pageable buffers
/// would be staged through the driver anyway.
enum HostBuf {
    Pinned { ptr: *mut u8, len: usize },
    Heap(Vec<u8>),
}

impl HostBuf {
    fn new(len: usize) -> Self {
        if let Ok(rt) = cudart::runtime() {
            let mut p: *mut c_void = ptr::null_mut();
            let ok = unsafe {
                rt.cuda_host_alloc()
                    .map(|f| f(&mut p, len, 0))
                    .is_ok_and(|st| st.0 == 0 && !p.is_null())
            };
            if ok {
                return HostBuf::Pinned {
                    ptr: p as *mut u8,
                    len,
                };
            }
        }
        HostBuf::Heap(vec![0u8; len])
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        match self {
            HostBuf::Pinned { ptr, .. } => *ptr,
            HostBuf::Heap(v) => v.as_mut_ptr(),
        }
    }

    fn as_ptr(&self) -> *const u8 {
        match self {
            HostBuf::Pinned { ptr, .. } => *ptr as *const u8,
            HostBuf::Heap(v) => v.as_ptr(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            HostBuf::Pinned { ptr, len } => unsafe { std::slice::from_raw_parts(*ptr, *len) },
            HostBuf::Heap(v) => v.as_slice(),
        }
    }
}

impl Drop for HostBuf {
    fn drop(&mut self) {
        if let HostBuf::Pinned { ptr, .. } = self
            && let Ok(rt) = cudart::runtime()
            && let Ok(f) = rt.cuda_free_host()
        {
            unsafe {
                let _ = f(*ptr as *mut c_void);
            }
        }
    }
}

/// nvJPEG handle plus per-process state (decode state, encoder state,
/// encoder parameters) and a dedicated CUDA stream for async work.
#[derive(Debug)]
struct Inner {
    lib: &'static nvjpeg::Nvjpeg,
    handle: nvjpeg::nvjpegHandle_t,
    state: nvjpeg::nvjpegJpegState_t,
    encoder_state: nvjpeg::nvjpegEncoderState_t,
    encoder_params: nvjpeg::nvjpegEncoderParams_t,
    stream: cudaStream_t,
}

// SAFETY: every field is only dereferenced while the handle is borrowed
// from `CudaTranscoder`'s pool, so no raw handle is ever touched by two
// threads at once — mirroring how the C library expects to be used.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        // Intentionally a no-op. The nvjpeg destroy functions can throw a
        // C++ exception through the `extern "C"` boundary when the driver is
        // degraded (observed with `nvjpegEncoderParamsDestroy` after a failed
        // encode: `terminate` is uncatchable from Rust), so teardown is left
        // to the OS, which reclaims the CUDA context at process exit.
    }
}

/// Create a dedicated CUDA stream; `null` (the synchronous default stream)
/// when the CUDA runtime cannot be loaded.
fn create_stream() -> cudaStream_t {
    if let Ok(rt) = cudart::runtime() {
        let mut stream: cudaStream_t = ptr::null_mut();
        let ok = unsafe {
            rt.cuda_stream_create()
                .map(|f| f(&mut stream))
                .is_ok_and(|st| st.0 == 0)
        };
        if ok {
            return stream;
        }
    }
    ptr::null_mut()
}

/// Wait for everything enqueued on the stream (no-op for the null stream,
/// whose calls are synchronous).
unsafe fn sync_stream(stream: cudaStream_t) -> Result<(), TranscodeError> {
    if stream.is_null() {
        return Ok(());
    }
    let pfn = cudart::runtime()
        .map_err(|e| TranscodeError::Gpu(format!("cuda runtime: {e}")))?
        .cuda_stream_synchronize()
        .map_err(|e| TranscodeError::Gpu(format!("cudaStreamSynchronize: {e}")))?;
    check_cuda(unsafe { pfn(stream) })
}

impl Inner {
    /// Create one nvjpeg handle with decode state, encoder state, encoder
    /// parameters, and a dedicated CUDA stream. Fails (as
    /// [`TranscodeError::Unavailable`]) when the library or driver is not
    /// present; callers fall back to the CPU backend.
    fn new() -> Result<Self, TranscodeError> {
        let lib = nvjpeg::nvjpeg()
            .map_err(|e| TranscodeError::Unavailable(format!("nvJPEG library: {e}")))?;

        unsafe {
            let mut handle = ptr::null_mut();
            let pfn = lib
                .nvjpeg_create_simple()
                .map_err(|e| TranscodeError::Unavailable(format!("nvjpegCreateSimple: {e}")))?;
            check(pfn(&mut handle))?;

            let mut state = ptr::null_mut();
            let pfn = lib
                .nvjpeg_jpeg_state_create()
                .map_err(|e| TranscodeError::Unavailable(format!("nvjpegJpegStateCreate: {e}")))?;
            if let Err(e) = check(pfn(handle, &mut state)) {
                if let Ok(destroy) = lib.nvjpeg_destroy() {
                    let _ = destroy(handle);
                }
                return Err(e);
            }

            let mut encoder_state = ptr::null_mut();
            let pfn = lib.nvjpeg_encoder_state_create().map_err(|e| {
                TranscodeError::Unavailable(format!("nvjpegEncoderStateCreate: {e}"))
            })?;
            if let Err(e) = check(pfn(handle, &mut encoder_state, ptr::null_mut())) {
                if let Ok(destroy) = lib.nvjpeg_jpeg_state_destroy() {
                    let _ = destroy(state);
                }
                if let Ok(destroy) = lib.nvjpeg_destroy() {
                    let _ = destroy(handle);
                }
                return Err(e);
            }

            let mut encoder_params = ptr::null_mut();
            let pfn = lib.nvjpeg_encoder_params_create().map_err(|e| {
                TranscodeError::Unavailable(format!("nvjpegEncoderParamsCreate: {e}"))
            })?;
            if let Err(e) = check(pfn(handle, &mut encoder_params, ptr::null_mut())) {
                if let Ok(destroy) = lib.nvjpeg_encoder_state_destroy() {
                    let _ = destroy(encoder_state);
                }
                if let Ok(destroy) = lib.nvjpeg_jpeg_state_destroy() {
                    let _ = destroy(state);
                }
                if let Ok(destroy) = lib.nvjpeg_destroy() {
                    let _ = destroy(handle);
                }
                return Err(e);
            }

            Ok(Self {
                lib,
                handle,
                state,
                encoder_state,
                encoder_params,
                stream: create_stream(),
            })
        }
    }

    /// Transcode one stream. The decode (for JPEG input) and encode are
    /// enqueued on [`Self::stream`] back to back, so the whole image is one
    /// async chain that overlaps with other handles' streams.
    fn transcode(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        match input {
            Input::Jpeg(bytes) => {
                if jpeg_components(bytes) == Some(1) {
                    return Err(TranscodeError::Encode(
                        "nvJPEG has no single-channel input format; the CPU backend \
                         preserves /DeviceGray JPEGs"
                            .into(),
                    ));
                }
                let (w, h, subsampling, mut buf) = unsafe { self.decode(bytes) }?;
                match subsampling {
                    CSS_420 | CSS_422 | CSS_444 => unsafe {
                        self.encode_yuv(w, h, subsampling, &buf, quality)
                    },
                    _ => unsafe {
                        self.encode_rgb(w, h, buf.as_mut_ptr(), (w * 3) as usize, quality)
                    },
                }
            }
            Input::Pixels(ImageRef::Luma8 { .. }) => Err(TranscodeError::Encode(
                "nvJPEG has no single-channel input format; the CPU backend encodes \
                 /DeviceGray streams"
                    .into(),
            )),
            Input::Pixels(ImageRef::Rgb8 {
                width,
                height,
                bytes,
            }) => unsafe {
                self.encode_rgb(
                    *width,
                    *height,
                    bytes.as_ptr() as *mut _,
                    (*width * 3) as usize,
                    quality,
                )
            },
        }
    }

    /// Enqueue a JPEG decode on the stream (async). Returns the dimensions,
    /// the source chroma subsampling, and a pinned host buffer holding either
    /// planar YUV (for 4:2:0/4:2:2/4:4:4 sources) or interleaved RGB.
    ///
    /// The buffer must not be read until the stream is synchronized, which
    /// happens inside the follow-up encode call.
    unsafe fn decode(&self, data: &[u8]) -> Result<(u32, u32, c_int, HostBuf), TranscodeError> {
        let mut n_components: c_int = 0;
        let mut subsampling: c_int = 0;
        let mut widths = [0i32; 3];
        let mut heights = [0i32; 3];
        let pfn = self
            .lib
            .nvjpeg_get_image_info()
            .map_err(|e| TranscodeError::Unavailable(format!("nvjpegGetImageInfo: {e}")))?;
        check(unsafe {
            pfn(
                self.handle,
                data.as_ptr(),
                data.len(),
                &mut n_components,
                &mut subsampling,
                widths.as_mut_ptr(),
                heights.as_mut_ptr(),
            )
        })?;

        let (w, h) = (widths[0] as usize, heights[0] as usize);
        let pfn = self
            .lib
            .nvjpeg_decode()
            .map_err(|e| TranscodeError::Unavailable(format!("nvjpegDecode: {e}")))?;

        // Planar YUV when the source is a 3-component 4:2:0/4:2:2/4:4:4 JPEG:
        // half the pixels of RGB, no colorspace round-trip, and the encoder
        // consumes it directly. Everything else decodes to interleaved RGB.
        if n_components == 3 && matches!(subsampling, CSS_420 | CSS_422 | CSS_444) {
            let (uw, uh) = match subsampling {
                CSS_422 => (w.div_ceil(2), h),
                CSS_420 => (w.div_ceil(2), h.div_ceil(2)),
                _ => (w, h),
            };
            let mut buf = HostBuf::new(w * h + 2 * uw * uh);
            let base = buf.as_mut_ptr();
            let mut image = nvjpeg::nvjpegImage_t {
                channel: unsafe {
                    [
                        base,
                        base.add(w * h),
                        base.add(w * h + uw * uh),
                        ptr::null_mut(),
                    ]
                },
                pitch: [w, uw, uw, 0],
            };
            check(unsafe {
                pfn(
                    self.handle,
                    self.state,
                    data.as_ptr(),
                    data.len(),
                    nvjpeg::nvjpegOutputFormat_t::Yuv,
                    &mut image,
                    self.stream,
                )
            })?;
            Ok((w as u32, h as u32, subsampling, buf))
        } else {
            let mut buf = HostBuf::new(w * h * 3);
            let mut image = nvjpeg::nvjpegImage_t {
                channel: [
                    buf.as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ],
                pitch: [w * 3, 0, 0, 0],
            };
            check(unsafe {
                pfn(
                    self.handle,
                    self.state,
                    data.as_ptr(),
                    data.len(),
                    nvjpeg::nvjpegOutputFormat_t::Rgbi,
                    &mut image,
                    self.stream,
                )
            })?;
            Ok((w as u32, h as u32, subsampling, buf))
        }
    }

    /// Encode planar YUV pixels (source subsampling preserved) to a JPEG
    /// bitstream, enqueued on the stream after the decode.
    unsafe fn encode_yuv(
        &self,
        width: u32,
        height: u32,
        subsampling: c_int,
        buf: &HostBuf,
        quality: u8,
    ) -> Result<Vec<u8>, TranscodeError> {
        let (w, h) = (width as usize, height as usize);
        let (uw, uh) = match subsampling {
            CSS_422 => (w.div_ceil(2), h),
            CSS_420 => (w.div_ceil(2), h.div_ceil(2)),
            _ => (w, h),
        };
        let base = buf.as_ptr() as *mut u8;
        let image = nvjpeg::nvjpegImage_t {
            channel: unsafe {
                [
                    base,
                    base.add(w * h),
                    base.add(w * h + uw * uh),
                    ptr::null_mut(),
                ]
            },
            pitch: [w, uw, uw, 0],
        };
        let subsampling = match subsampling {
            CSS_422 => nvjpeg::nvjpegChromaSubsampling_t::Css422,
            CSS_444 => nvjpeg::nvjpegChromaSubsampling_t::Css444,
            _ => nvjpeg::nvjpegChromaSubsampling_t::Css420,
        };

        unsafe {
            self.set_encoder_params(quality)?;
            let pfn = self
                .lib
                .nvjpeg_encode_yuv()
                .map_err(|e| TranscodeError::Unavailable(format!("nvjpegEncodeYUV: {e}")))?;
            check(pfn(
                self.handle,
                self.encoder_state,
                self.encoder_params,
                &image,
                subsampling,
                width as c_int,
                height as c_int,
                self.stream,
            ))?;
            self.retrieve()
        }
    }

    /// Encode interleaved RGB pixels to a JPEG bitstream on the stream.
    /// `src` may be the decode buffer or a raw PDF stream's bytes; it must
    /// stay valid until the stream is synchronized (inside [`Self::retrieve`]).
    unsafe fn encode_rgb(
        &self,
        width: u32,
        height: u32,
        src: *mut u8,
        pitch: usize,
        quality: u8,
    ) -> Result<Vec<u8>, TranscodeError> {
        let image = nvjpeg::nvjpegImage_t {
            channel: [src, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()],
            pitch: [pitch, 0, 0, 0],
        };
        unsafe {
            self.set_encoder_params(quality)?;
            let pfn = self
                .lib
                .nvjpeg_encode_image()
                .map_err(|e| TranscodeError::Unavailable(format!("nvjpegEncodeImage: {e}")))?;
            check(pfn(
                self.handle,
                self.encoder_state,
                self.encoder_params,
                &image,
                NVJPEG_INPUT_RGBI,
                width as c_int,
                height as c_int,
                self.stream,
            ))?;
            self.retrieve()
        }
    }

    /// Configure the shared encoder params for one encode.
    ///
    /// Freshly-created encoder params carry an invalid default subsampling
    /// state and make the encode calls return `NVJPEG_STATUS_INVALID_PARAMETER`
    /// (observed on CUDA 13.3), so the sampling factors must be set
    /// explicitly before every encode.
    unsafe fn set_encoder_params(&self, quality: u8) -> Result<(), TranscodeError> {
        let pfn = self.lib.nvjpeg_encoder_params_set_quality().map_err(|e| {
            TranscodeError::Unavailable(format!("nvjpegEncoderParamsSetQuality: {e}"))
        })?;
        check(unsafe { pfn(self.encoder_params, quality as c_int, ptr::null_mut()) })?;

        let pfn = self
            .lib
            .nvjpeg_encoder_params_set_sampling_factors()
            .map_err(|e| {
                TranscodeError::Unavailable(format!("nvjpegEncoderParamsSetSamplingFactors: {e}"))
            })?;
        check(unsafe {
            pfn(
                self.encoder_params,
                nvjpeg::nvjpegChromaSubsampling_t::Css420,
                ptr::null_mut(),
            )
        })?;

        let pfn = self
            .lib
            .nvjpeg_encoder_params_set_optimized_huffman()
            .map_err(|e| {
                TranscodeError::Unavailable(format!("nvjpegEncoderParamsSetOptimizedHuffman: {e}"))
            })?;
        check(unsafe { pfn(self.encoder_params, 1, ptr::null_mut()) })?;
        Ok(())
    }

    /// Wait for the enqueued decode+encode, then copy the bitstream out.
    /// Everything stays on the handle's own stream so it never serializes
    /// against other handles' work (the legacy null stream would).
    unsafe fn retrieve(&self) -> Result<Vec<u8>, TranscodeError> {
        // Query the bitstream size on the stream, then read it.
        let pfn = self.lib.nvjpeg_encode_retrieve_bitstream().map_err(|e| {
            TranscodeError::Unavailable(format!("nvjpegEncodeRetrieveBitstream: {e}"))
        })?;
        let mut needed = 0usize;
        check(unsafe {
            pfn(
                self.handle,
                self.encoder_state,
                ptr::null_mut(),
                &mut needed,
                self.stream,
            )
        })?;
        unsafe { sync_stream(self.stream)? };
        if needed == 0 {
            return Err(TranscodeError::Encode(
                "nvjpeg produced an empty bitstream".into(),
            ));
        }

        let mut out = HostBuf::new(needed);
        let mut len = needed;
        check(unsafe {
            pfn(
                self.handle,
                self.encoder_state,
                out.as_mut_ptr(),
                &mut len,
                self.stream,
            )
        })?;
        unsafe { sync_stream(self.stream)? };
        Ok(out.as_slice()[..len.min(needed)].to_vec())
    }
}

/// NVIDIA nvJPEG transcoder.
///
/// Holds a pool of independent [`Inner`] handles, each with its own CUDA
/// stream: every rayon worker pops one for the duration of a call, so many
/// streams transcode on the GPU concurrently without ever touching another
/// worker's handle. The pool grows lazily up to the number of concurrent
/// callers.
#[derive(Debug)]
pub struct CudaTranscoder {
    pool: Mutex<Vec<Inner>>,
}

impl CudaTranscoder {
    /// Create the pool, pre-warming one handle so the first stream does not
    /// pay the full CUDA context-init latency. Fails (as
    /// [`TranscodeError::Unavailable`]) when the library or driver is not
    /// present; callers fall back to the CPU backend.
    pub fn new() -> Result<Self, TranscodeError> {
        Ok(Self {
            pool: Mutex::new(vec![Inner::new()?]),
        })
    }
}

impl ImageTranscoder for CudaTranscoder {
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        // Check the stream class cheaply before touching the pool so a
        // grayscale stream never borrows a handle (it will be re-routed to
        // the CPU backend anyway).
        if matches!(
            input,
            Input::Jpeg(b) if jpeg_components(b) == Some(1)
        ) || matches!(input, Input::Pixels(ImageRef::Luma8 { .. }))
        {
            return Err(TranscodeError::Encode(
                "nvJPEG has no single-channel input format; the CPU backend handles \
                 /DeviceGray streams"
                    .into(),
            ));
        }

        // Take an idle handle (or create one — serialized under the pool
        // lock, so at most one is created per lock acquisition). The handle
        // is returned before the Result is inspected, so the pool never
        // leaks handles on error paths.
        let inner = match self.pool.lock().unwrap().pop() {
            Some(inner) => inner,
            None => Inner::new()?,
        };
        let result = inner.transcode(input, quality);
        self.pool.lock().unwrap().push(inner);
        result
    }
}
