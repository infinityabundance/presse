//! NVIDIA nvJPEG backend (`--acceleration cuda`, feature = "cuda").
//!
//! Uses [`baracuda-nvjpeg-sys`]'s dynamic loader, which dlopens
//! `libnvjpeg.so`/`nvjpeg64.dll` at runtime — no CUDA toolkit needed to
//! build, no link-time driver dependency. Any failure (library missing,
//! driver missing, context/VRAM errors, per-stream errors) surfaces as
//! [`TranscodeError`] and the caller's [`FallbackTranscoder`] degrades to
//! the CPU backend.
//!
//! The decode path targets plain host memory via the synchronous
//! `nvjpegDecode` API; the encode path uses `nvjpegEncodeImage` +
//! `nvjpegEncodeRetrieveBitstream` with interleaved RGB input. nvJPEG has no
//! single-channel input format, so `/DeviceGray` streams are routed back to
//! the CPU backend. nvJPEG states are not thread-safe, so the handle is
//! serialized behind a mutex; CUDA-stream parallelism is left as a future
//! optimization.
//!
//! **Validation status:** decode + encode were exercised on NVIDIA hardware
//! (RTX 4080 SUPER, CUDA 13.3). Fresh encoder params reject
//! `nvjpegEncodeImage` with `NVJPEG_STATUS_INVALID_PARAMETER` until
//! subsampling is set explicitly, so [`Inner::encode`] always configures 4:2:0
//! sampling (and optimized Huffman tables). The fallback paths are also
//! covered by the regression suite without a GPU.

use baracuda_nvjpeg_sys as nvjpeg;

use crate::transcode::{ImageRef, ImageTranscoder, Input, TranscodeError, jpeg_components};
use std::ffi::c_int;
use std::ptr;
use std::sync::Mutex;

/// `nvjpegInputFormat_t` value for `nvjpegEncodeImage`.
///
/// Only interleaved RGB is supported here: nvJPEG exposes no grayscale input
/// format (`NVJPEG_INPUT_RGBI` is `5`; older headers also had a `Y` at `2`,
/// which CUDA 13.3 removed), so `/DeviceGray` streams fall back to the CPU
/// backend.
const NVJPEG_INPUT_RGBI: c_int = 5; // interleaved RGB

fn check(status: nvjpeg::nvjpegStatus_t) -> Result<(), TranscodeError> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(TranscodeError::Gpu(format!("nvjpeg status {}", status.0)))
    }
}

/// nvJPEG handle plus per-process state (decode state, encoder state, and
/// encoder parameters). Serialized behind a mutex: nvjpeg objects are not
/// safe for concurrent use.
#[derive(Debug)]
struct Inner {
    lib: &'static nvjpeg::Nvjpeg,
    handle: nvjpeg::nvjpegHandle_t,
    state: nvjpeg::nvjpegJpegState_t,
    encoder_state: nvjpeg::nvjpegEncoderState_t,
    encoder_params: nvjpeg::nvjpegEncoderParams_t,
}

// SAFETY: every field is only dereferenced inside `CudaTranscoder.inner`, a
// mutex, so the raw handles are never touched concurrently — mirroring how
// the C library expects to be used.
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

impl Inner {
    /// Decode a JPEG stream to interleaved RGB host memory.
    unsafe fn decode(&self, data: &[u8]) -> Result<(u32, u32, Vec<u8>), TranscodeError> {
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
        let mut out = vec![0u8; w * h * 3];
        let mut image = nvjpeg::nvjpegImage_t {
            channel: [
                out.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            ],
            pitch: [w * 3, 0, 0, 0],
        };
        let pfn = self
            .lib
            .nvjpeg_decode()
            .map_err(|e| TranscodeError::Unavailable(format!("nvjpegDecode: {e}")))?;
        check(unsafe {
            pfn(
                self.handle,
                self.state,
                data.as_ptr(),
                data.len(),
                nvjpeg::nvjpegOutputFormat_t::Rgbi,
                &mut image,
                ptr::null_mut(), // default CUDA stream
            )
        })?;
        Ok((w as u32, h as u32, out))
    }

    /// Encode interleaved RGB pixels to a JPEG bitstream.
    ///
    /// Freshly-created encoder params carry an invalid default subsampling
    /// state and make `nvjpegEncodeImage` return
    /// `NVJPEG_STATUS_INVALID_PARAMETER` (observed on CUDA 13.3), so the
    /// sampling factors must be set explicitly before every encode.
    unsafe fn encode(
        &self,
        width: u32,
        height: u32,
        bytes: &[u8],
        pitch: usize,
        quality: u8,
    ) -> Result<Vec<u8>, TranscodeError> {
        let image = nvjpeg::nvjpegImage_t {
            channel: [
                bytes.as_ptr() as *mut _,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            ],
            pitch: [pitch, 0, 0, 0],
        };

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

        let pfn = self
            .lib
            .nvjpeg_encode_image()
            .map_err(|e| TranscodeError::Unavailable(format!("nvjpegEncodeImage: {e}")))?;
        check(unsafe {
            pfn(
                self.handle,
                self.encoder_state,
                self.encoder_params,
                &image,
                NVJPEG_INPUT_RGBI,
                width as c_int,
                height as c_int,
                ptr::null_mut(),
            )
        })?;

        // Query the bitstream size, then copy it out.
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
                ptr::null_mut(),
            )
        })?;
        if needed == 0 {
            return Err(TranscodeError::Encode(
                "nvjpeg produced an empty bitstream".into(),
            ));
        }
        let mut out = vec![0u8; needed];
        let mut len = needed;
        check(unsafe {
            pfn(
                self.handle,
                self.encoder_state,
                out.as_mut_ptr(),
                &mut len,
                ptr::null_mut(),
            )
        })?;
        out.truncate(len);
        Ok(out)
    }
}

/// NVIDIA nvJPEG transcoder.
#[derive(Debug)]
pub struct CudaTranscoder {
    inner: Mutex<Inner>,
}

impl CudaTranscoder {
    /// Load libnvjpeg and create the handle + states. Fails (as
    /// [`TranscodeError::Unavailable`]) when the library or driver is not
    /// present; callers fall back to the CPU backend.
    pub fn new() -> Result<Self, TranscodeError> {
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
                inner: Mutex::new(Inner {
                    lib,
                    handle,
                    state,
                    encoder_state,
                    encoder_params,
                }),
            })
        }
    }
}

impl ImageTranscoder for CudaTranscoder {
    fn transcode_image(&self, input: &Input, quality: u8) -> Result<Vec<u8>, TranscodeError> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            match input {
                Input::Jpeg(bytes) => {
                    if jpeg_components(bytes) == Some(1) {
                        return Err(TranscodeError::Encode(
                            "nvJPEG has no single-channel input format; the CPU backend \
                             preserves /DeviceGray JPEGs"
                                .into(),
                        ));
                    }
                    let (w, h, rgb) = inner.decode(bytes)?;
                    inner.encode(w, h, &rgb, (w * 3) as usize, quality)
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
                }) => inner.encode(*width, *height, bytes, (*width * 3) as usize, quality),
            }
        }
    }
}
