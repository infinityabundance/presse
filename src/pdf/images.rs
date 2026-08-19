use lopdf::{Document, Object, ObjectId, Stream};
use rayon::prelude::*;

use crate::transcode::{
    CpuTranscoder, ImageRef, ImageTranscoder, Input, TranscodeCache, encode_key,
};

/// Fixed-size block used for raw pixel buffer processing.
/// 32 KiB keeps the working set inside the L1/L2 caches (and aligned with
/// 4-byte pixels), so multi-MB scans are walked in cache-resident slices
/// instead of one long linear sweep.
const CHUNK_SIZE: usize = 32 * 1024;

/// Replace JPEG images in a document by a compressed version to the given quality.
/// Only JPEG images are replaced, the other are skipped.
///
/// Uses the default CPU backend (see [`compress_images_with`]).
pub fn compress_images(doc: &mut Document, quality: u8, verbose: bool) {
    compress_images_with(doc, quality, verbose, &CpuTranscoder);
}

/// Replace JPEG images in a document using the given transcoding backend.
/// Only JPEG images are replaced, the other are skipped.
///
/// The pipeline is split in three phases:
/// 1. **Extract** — detach every eligible image stream from the `Document`
///    object tree (serial, cheap: just map lookups + moves, no copies).
/// 2. **Re-encode** — transcode all streams concurrently with rayon on owned,
///    detached buffers. No `Document` state is read or written from worker
///    threads, so there is nothing to lock.
/// 3. **Apply** — write the re-encoded streams (and updated `/Filter` +
///    `/Length` dictionary entries) back into the object tree in a single
///    serial pass, right before serialization.
pub fn compress_images_with<T: ImageTranscoder>(
    doc: &mut Document,
    quality: u8,
    verbose: bool,
    transcoder: &T,
) {
    // Phase 1 — extract.
    let image_ids: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, obj)| match obj {
            Object::Stream(stream) if is_image_stream(stream) => Some(*id),
            _ => None,
        })
        .collect();

    let mut images: Vec<(ObjectId, Stream)> = Vec::with_capacity(image_ids.len());
    for id in image_ids {
        if let Some(Object::Stream(stream)) = doc.objects.remove(&id) {
            images.push((id, stream));
        }
    }

    verbose!(verbose, "[images] Found {} image object(s)", images.len());

    // Deduplication cache: identical images (logos, watermarks, headers)
    // appear dozens of times in real PDFs; each unique image is encoded
    // exactly once (competing workers converge on one result).
    let cache = TranscodeCache::new();

    // Phase 2 — re-encode in parallel. Workers never touch `Document` state
    // (document-lock-free); the dedup cache is the only shared structure and
    // is internally synchronized.
    let reencoded: Vec<(ObjectId, Stream)> = images
        .into_par_iter()
        .map(|(id, stream)| {
            let stream = reencode_image_stream(id, stream, quality, verbose, &cache, transcoder);
            (id, stream)
        })
        .collect();

    // Phase 3 — single mutation pass back onto the document.
    for (id, stream) in reencoded {
        doc.objects.insert(id, Object::Stream(stream));
    }
}

/// Re-encode a single image stream. The stream is owned and detached from the
/// document, so this can run on any rayon worker thread.
///
/// Returns the stream that should be stored back: the original, untouched,
/// when nothing could or should be re-encoded; a modified copy carrying the
/// new JPEG payload and updated `/Filter` + `/Length` entries otherwise.
fn reencode_image_stream<T: ImageTranscoder>(
    id: ObjectId,
    mut stream: Stream,
    quality: u8,
    verbose: bool,
    cache: &TranscodeCache,
    transcoder: &T,
) -> Stream {
    let color_space_raw = stream
        .dict
        .get(b"ColorSpace")
        .and_then(|f| f.as_name())
        .ok();
    let width = stream
        .dict
        .get(b"Width")
        .and_then(|w| w.as_i64())
        .unwrap_or(0) as u32;
    let height = stream
        .dict
        .get(b"Height")
        .and_then(|h| h.as_i64())
        .unwrap_or(0) as u32;

    // Detect filter: may be a Name or an Array
    let filter_name = stream.dict.get(b"Filter").ok();
    let filter_str = filter_name.map(|f| match f {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(arr) => {
            let names: Vec<String> = arr
                .iter()
                .filter_map(|e| e.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect();
            format!("[{}]", names.join(", "))
        }
        _ => format!("{:?}", f),
    });
    let color_str = color_space_raw
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    verbose!(
        verbose,
        "[img {:?}] filter={} colorspace={} size={}x{} raw_content={}B",
        id,
        filter_str.as_deref().unwrap_or("none"),
        color_str,
        width,
        height,
        stream.content.len()
    );

    // CMYK images not yet supported by image crate
    if color_space_raw == Some(b"DeviceCMYK") {
        verbose!(verbose, "[img {:?}] → skipped: CMYK not supported", id);
        return stream;
    }

    // Resolve the effective image filter.
    // Singleton --> ok.
    // Multi-element Array --> (e.g. [FlateDecode, DCTDecode]) not supported, skip.
    let filter: Option<&[u8]> = match stream.dict.get(b"Filter").ok() {
        Some(Object::Name(n)) => Some(n.as_slice()),
        Some(Object::Array(arr)) if arr.len() == 1 => arr[0].as_name().ok(),
        Some(Object::Array(arr)) => {
            if verbose {
                let names: Vec<String> = arr
                    .iter()
                    .filter_map(|e| e.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .collect();
                eprintln!(
                    "[img {:?}] → skipped: multi-filter pipeline [{}] not supported",
                    id,
                    names.join(", ")
                );
            }
            return stream;
        }
        _ => None,
    };

    let buf: Vec<u8> = match filter {
        Some(b"DCTDecode") | Some(b"JPXDecode") => {
            verbose!(verbose, "[img {:?}] → processing JPEG/JPX via backend", id);
            // Identical JPEG bytes re-encode identically: encode once, reuse
            // the cached buffer for every duplicate image object.
            let input = Input::Jpeg(&stream.content);
            let key = encode_key(quality, 2, width, height, &stream.content);
            let result = cache.get(key, || transcoder.transcode_image(&input, quality));
            match result.as_ref() {
                Ok(buf) if !buf.is_empty() => buf.clone(),
                Ok(_) => {
                    verbose!(
                        verbose,
                        "[img {:?}] → skipped: backend produced empty output",
                        id
                    );
                    return stream;
                }
                Err(e) => {
                    verbose!(verbose, "[img {:?}] → skipped: transcode failed: {}", id, e);
                    return stream;
                }
            }
        }
        Some(other_filter) => {
            verbose!(
                verbose,
                "[img {:?}] → processing raw pixels (filter={}, colorspace={})",
                id,
                String::from_utf8_lossy(other_filter),
                color_str
            );
            let raw = match stream.decompressed_content() {
                Ok(data) => data,
                Err(e) => {
                    verbose!(
                        verbose,
                        "[img {:?}] → skipped: decompressed_content failed: {}",
                        id,
                        e
                    );
                    return stream;
                }
            };

            let classified = match classify_raw(&raw, width, height, color_space_raw) {
                Some(c) => c,
                None => {
                    verbose!(
                        verbose,
                        "[img {:?}] → skipped: mismatched dimensions or unsupported format, expected {}x{}x{} bytes",
                        id,
                        width,
                        height,
                        if color_space_raw == Some(b"DeviceGray") {
                            width * height
                        } else {
                            width * height * 3
                        }
                    );
                    return stream;
                }
            };
            let (input, key_bytes, tag): (Input<'_>, &[u8], u8) = match &classified {
                RawPixels::Luma(bytes) => (
                    Input::Pixels(ImageRef::Luma8 {
                        width,
                        height,
                        bytes,
                    }),
                    bytes,
                    1,
                ),
                RawPixels::Rgb(bytes) => (
                    Input::Pixels(ImageRef::Rgb8 {
                        width,
                        height,
                        bytes,
                    }),
                    bytes,
                    3,
                ),
                RawPixels::RgbaNormalized(rgb) => (
                    Input::Pixels(ImageRef::Rgb8 {
                        width,
                        height,
                        bytes: rgb.as_slice(),
                    }),
                    rgb.as_slice(),
                    3,
                ),
            };

            // Same pixels → same JPEG, regardless of how the source stream
            // happened to be compressed. Width/height are keyed too: two
            // images with identical pixel bytes but different dimensions
            // must not share a cached JPEG.
            let key = encode_key(quality, tag, width, height, key_bytes);
            let result = cache.get(key, || transcoder.transcode_image(&input, quality));
            match result.as_ref() {
                Ok(buf) if !buf.is_empty() => buf.clone(),
                Ok(_) => {
                    verbose!(
                        verbose,
                        "[img {:?}] → skipped: backend produced empty output",
                        id
                    );
                    return stream;
                }
                Err(e) => {
                    verbose!(verbose, "[img {:?}] → skipped: transcode failed: {}", id, e);
                    return stream;
                }
            }
        }
        None => {
            verbose!(
                verbose,
                "[img {:?}] → skipped: no Filter entry (uncompressed stream)",
                id
            );
            return stream;
        }
    };

    if buf.is_empty() {
        verbose!(
            verbose,
            "[img {:?}] → skipped: JPEG encoding produced empty output",
            id
        );
        return stream;
    }

    if buf.len() < stream.content.len() {
        verbose!(
            verbose,
            "[img {:?}] → compressed {}B → {}B",
            id,
            stream.content.len(),
            buf.len()
        );
        stream.content = buf;
        stream
            .dict
            .set(b"Filter", Object::Name(b"DCTDecode".to_vec()));
        stream
            .dict
            .set(b"Length", Object::Integer(stream.content.len() as i64));
    } else {
        verbose!(
            verbose,
            "[img {:?}] → skipped: re-encoded ({}B) not smaller than original ({}B)",
            id,
            buf.len(),
            stream.content.len()
        );
    }

    stream
}

/// A raw (decompressed) pixel buffer, classified into a backend-friendly
/// layout.
enum RawPixels<'a> {
    /// 1 byte/pixel grayscale.
    Luma(&'a [u8]),
    /// 3 bytes/pixel interleaved RGB.
    Rgb(&'a [u8]),
    /// A 4-byte/pixel `DeviceRGB` stream normalized to RGB by dropping the
    /// alpha channel in fixed-size chunks (owned result).
    RgbaNormalized(Vec<u8>),
}

/// Classify a decompressed pixel buffer.
///
/// Canonical layouts are wrapped zero-copy: `DeviceGray` → 1 byte/pixel,
/// `DeviceRGB` → 3 bytes/pixel. Streams that illegally carry a fourth (alpha)
/// byte per pixel on a `DeviceRGB` stream are normalized by dropping the
/// alpha channel in fixed-size chunks (each 32 KiB chunk holds a whole number
/// of 4-byte pixels, so a pixel never straddles a chunk boundary). Anything
/// else is rejected (`None` → the stream is skipped by the caller).
fn classify_raw<'a>(
    raw: &'a [u8],
    width: u32,
    height: u32,
    color_space: Option<&[u8]>,
) -> Option<RawPixels<'a>> {
    let (w, h) = (width as usize, height as usize);
    let area = w.checked_mul(h)?;

    match color_space {
        Some(b"DeviceGray") if raw.len() == area => Some(RawPixels::Luma(raw)),
        _ if raw.len() == area.checked_mul(3)? => Some(RawPixels::Rgb(raw)),
        _ if raw.len() == area.checked_mul(4)? => {
            let mut rgb = Vec::with_capacity(area * 3);
            for chunk in raw.chunks(CHUNK_SIZE) {
                for px in chunk.chunks_exact(4) {
                    rgb.extend_from_slice(&px[..3]);
                }
            }
            Some(RawPixels::RgbaNormalized(rgb))
        }
        _ => None,
    }
}

/// Check if a stream represents an image.
fn is_image_stream(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .and_then(|s| s.as_name())
        .ok()
        .is_some_and(|name| name == b"Image")
}
