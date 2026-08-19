//! Regression tests for the image re-encoding pipeline (`compress_images`).
//!
//! Three layers of verification:
//! 1. **Structure & syntax** — every output PDF must reload cleanly with
//!    `lopdf`, keep consistent `/Length` entries on every stream, and pass
//!    ghostscript's parser when `gs` is available.
//! 2. **Visual diff** — pages are rasterized with `pdftoppm` before/after
//!    compression and compared with SSIM, so any color-space corruption or
//!    stream damage shows up as a large drop. These tests skip gracefully
//!    when poppler-utils is not installed.
//! 3. **Edge cases** — zero-image PDFs, CMYK streams, inline images, mixed
//!    `FlateDecode` streams, non-canonical 4-byte/px `DeviceRGB` streams, and
//!    serial/parallel determinism.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::GrayImage;
use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use presse::pdf::images::{QualityMode, compress_images, compress_images_with};
use presse::pdf::writer::{compress_and_save_pdf, save_pdf};
use presse::transcode::{
    Acceleration, CpuTranscoder, FallbackTranscoder, ImageTranscoder, Input, RuntimeTranscoder,
    TranscodeError, resolve,
};

const QUALITY: u8 = 50;

// ---------------------------------------------------------------------------
// Test environment helpers
// ---------------------------------------------------------------------------

fn test_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("presse-regression-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// True when the `pdftoppm` binary (poppler-utils) can be executed.
fn pdftoppm_available() -> bool {
    Command::new("pdftoppm").arg("-v").output().is_ok()
}

/// True when ghostscript is installed.
fn gs_available() -> bool {
    Command::new("gs").arg("--version").output().is_ok()
}

/// True when qpdf is installed.
fn qpdf_available() -> bool {
    Command::new("qpdf").arg("--version").output().is_ok()
}

/// CI convention shared with `ci/validate_corpus.py`: when
/// `PRESSE_REQUIRE_PDF_TOOLS` is set, a missing validator is a hard test
/// failure instead of a silent skip, so a broken runner image cannot quietly
/// degrade the suite. Locally (unset) the gates degrade with a loud warning.
fn require_pdf_tools() -> bool {
    std::env::var_os("PRESSE_REQUIRE_PDF_TOOLS").is_some_and(|v| !v.is_empty())
}

/// Assert that an external validator is present, or fail/skip per
/// [`require_pdf_tools`].
fn ensure_tool(name: &str, available: bool) -> bool {
    if available {
        return true;
    }
    let msg = format!("{name} not found — skipping the {name} validity gate");
    if require_pdf_tools() {
        panic!("{msg}; set PRESSE_REQUIRE_PDF_TOOLS=0 to skip locally");
    }
    eprintln!("note: {msg}");
    false
}

// ---------------------------------------------------------------------------
// Synthetic pixel generators
// ---------------------------------------------------------------------------

fn xorshift(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

/// Photo-like RGB: smooth gradient + structure + light grain.
fn photoish_rgb(w: u32, h: u32) -> Vec<u8> {
    let mut next = xorshift(42);
    let mut v = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = (
                (x as f32 / w as f32 * 255.0) as u8,
                (y as f32 / h as f32 * 255.0) as u8,
                (128.0 + 80.0 * ((x as f32 + y as f32) / 32.0).sin()) as u8,
            );
            let n = (next() & 0x1f) as u8;
            v.extend_from_slice(&[r.wrapping_add(n), g.wrapping_add(n), b.wrapping_add(n)]);
        }
    }
    v
}

fn gradient_gray(w: u32, h: u32) -> Vec<u8> {
    let mut next = xorshift(11);
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let base = (((x as f32 / w as f32) + (y as f32 / h as f32)) * 127.5) as u8;
            v.push(base.wrapping_add((next() & 0x0f) as u8));
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Document builders (all synthetic, in-memory, deterministic)
// ---------------------------------------------------------------------------

fn new_doc() -> (Document, lopdf::ObjectId) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => Vec::<Object>::new(),
            "Count" => 0,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    (doc, pages_id)
}

fn push_kid(doc: &mut Document, pages_id: lopdf::ObjectId, kid: lopdf::ObjectId) {
    if let Some(Object::Dictionary(pages)) = doc.objects.get_mut(&pages_id) {
        let mut kids: Vec<Object> = pages
            .get(b"Kids")
            .and_then(|k| k.as_array().cloned())
            .unwrap_or_default();
        kids.push(Object::Reference(kid));
        let count = kids.len() as u32;
        pages.set("Kids", kids);
        pages.set("Count", count);
    }
}

/// Add a page that draws `pixels` (w×h, `gray` = 1 or 3 bytes/px) full-bleed.
fn add_image_page(
    doc: &mut Document,
    pages_id: lopdf::ObjectId,
    pixels: Vec<u8>,
    w: u32,
    h: u32,
    gray: bool,
) {
    let mut image_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "BitsPerComponent" => 8,
    };
    image_dict.set("ColorSpace", if gray { "DeviceGray" } else { "DeviceRGB" });
    let mut image_stream = Stream::new(image_dict, pixels);
    image_stream.compress().unwrap(); // FlateDecode → re-encodable
    let image_id = doc.add_object(image_stream);

    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![w.into(), 0.into(), 0.into(), h.into(), 0.into(), 0.into()],
            ),
            Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    });
    push_kid(doc, pages_id, page_id);
}

/// Like [`add_image_page`], but drawn at `placed_w × placed_h` points
/// instead of at pixel size, so the effective resolution of the image can
/// be set independently of its raster (pixels ÷ (points/72) dpi).
fn add_image_page_at(
    doc: &mut Document,
    pages_id: lopdf::ObjectId,
    pixels: Vec<u8>,
    w: u32,
    h: u32,
    gray: bool,
    placed: (f64, f64),
) {
    let mut image_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "BitsPerComponent" => 8,
    };
    image_dict.set("ColorSpace", if gray { "DeviceGray" } else { "DeviceRGB" });
    let mut image_stream = Stream::new(image_dict, pixels);
    image_stream.compress().unwrap(); // FlateDecode → re-encodable
    let image_id = doc.add_object(image_stream);

    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    placed.0.into(),
                    0.into(),
                    0.into(),
                    placed.1.into(),
                    0.into(),
                    0.into(),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), placed.0.into(), placed.1.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    });
    push_kid(doc, pages_id, page_id);
}

fn add_text_page(doc: &mut Document, pages_id: lopdf::ObjectId, page_no: u32) {
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
            Operation::new("Td", vec![72.into(), 720.into()]),
            Operation::new(
                "Tj",
                vec![Object::String(
                    format!("Regression page {page_no}").into_bytes(),
                    lopdf::StringFormat::Literal,
                )],
            ),
            Operation::new("Td", vec![0.into(), (-16).into()]),
            Operation::new(
                "Tj",
                vec![Object::String(
                    b"Structural integrity must survive compression.".to_vec(),
                    lopdf::StringFormat::Literal,
                )],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        }}},
    });
    push_kid(doc, pages_id, page_id);
}

/// A page whose content stream embeds a 2×2 RGB **inline** image (BI..EI).
/// The stream is hand-crafted: inline image data is raw bytes after `ID`
/// (lopdf's operator encoder would wrap it in parentheses, which is not valid
/// inline-image syntax).
fn inline_image_doc() -> Document {
    let (mut doc, pages_id) = new_doc();
    // 2×2 RGB, no 0x45 ('E') / 0x49 ('I') byte inside so the raw `EI` marker
    // cannot be confused mid-stream.
    let inline: &[u8] = &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    let mut content = Vec::new();
    content.extend_from_slice(b"q 100 0 0 100 0 0 cm BI /W 2 /H 2 /BPC 8 /CS /RGB ID ");
    content.extend_from_slice(inline);
    content.extend_from_slice(b" EI Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        "Contents" => content_id,
        "Resources" => dictionary! {},
    });
    push_kid(&mut doc, pages_id, page_id);
    doc
}

/// A page with a `DeviceCMYK` DCTDecode image stream (must be skipped as-is).
fn cmyk_doc() -> Document {
    let (mut doc, pages_id) = new_doc();
    // Real JPEG payload — the point is that the compressor must skip it
    // (CMYK is not supported) and preserve it byte-for-byte.
    let mut payload = Vec::new();
    let img = image::RgbImage::from_raw(4, 4, vec![128u8; 4 * 4 * 3]).unwrap();
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut payload), 80)
        .encode_image(&img)
        .unwrap();
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 4,
            "Height" => 4,
            "ColorSpace" => "DeviceCMYK",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
            "Length" => payload.len() as i64,
        },
        payload,
    ));
    let content_id = doc.add_object(Stream::new(
        dictionary! {},
        Content { operations: vec![] }.encode().unwrap(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 64.into(), 64.into()],
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
        "Contents" => content_id,
    });
    push_kid(&mut doc, pages_id, page_id);
    doc
}

/// A page mixing a FlateDecode RGB image, a FlateDecode text stream and a
/// FlateDecode metadata stream.
fn mixed_flate_doc() -> Document {
    let (mut doc, pages_id) = new_doc();
    add_image_page(&mut doc, pages_id, photoish_rgb(96, 96), 96, 96, false);
    add_text_page(&mut doc, pages_id, 1);
    let mut metadata = Stream::new(
        dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
        b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF/></x:xmpmeta>".to_vec(),
    );
    metadata.compress().unwrap();
    doc.add_object(metadata);
    doc
}

/// A DeviceRGB image stream that illegally carries a 4th (alpha) byte per
/// pixel — the chunked-normalization path in `compress_images`.
fn rgba4_doc(w: u32, h: u32) -> Document {
    let (mut doc, pages_id) = new_doc();
    let mut next = xorshift(7);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        pixels.extend_from_slice(&[
            (next() & 0xff) as u8,
            (next() & 0xff) as u8,
            (next() & 0xff) as u8,
            255,
        ]);
    }
    add_image_page(&mut doc, pages_id, pixels, w, h, false);
    doc
}

/// The canonical (3 bytes/px) RGB counterpart of `rgba4_doc` — the ground
/// truth of what the 4-byte stream is supposed to look like.
fn rgb3_doc(w: u32, h: u32) -> Document {
    let (mut doc, pages_id) = new_doc();
    let mut next = xorshift(7);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..w * h {
        pixels.extend_from_slice(&[
            (next() & 0xff) as u8,
            (next() & 0xff) as u8,
            (next() & 0xff) as u8,
        ]);
    }
    add_image_page(&mut doc, pages_id, pixels, w, h, false);
    doc
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Assert the PDF at `path` loads cleanly, has ≥1 page, and every stream's
/// `/Length` matches its content exactly. Returns the loaded document.
fn assert_well_formed(path: &Path) -> Document {
    let doc = Document::load(path).unwrap_or_else(|e| panic!("output must load cleanly: {e}"));

    assert!(
        !doc.get_pages().is_empty(),
        "output must have at least one page"
    );

    for (id, obj) in doc.objects.iter() {
        if let Object::Stream(s) = obj {
            let len = s.dict.get(b"Length").ok().and_then(|l| l.as_i64().ok());
            if let Some(len) = len {
                assert_eq!(
                    len as usize,
                    s.content.len(),
                    "stream {id:?}: /Length {len} must equal content length {}",
                    s.content.len()
                );
            }
        }
    }

    if ensure_tool("gs", gs_available()) {
        let status = Command::new("gs")
            .args(["-sDEVICE=nullpage", "-dNOPAUSE", "-dBATCH", "-dQUIET"])
            .arg(path)
            .status()
            .expect("ghostscript should run");
        assert!(
            status.success(),
            "ghostscript rejected {} as malformed",
            path.display()
        );
    }

    // qpdf is the strictest syntax gate: it validates every xref entry.
    if ensure_tool("qpdf", qpdf_available()) {
        let output = Command::new("qpdf").args(["--check"]).arg(path).output();
        if let Ok(output) = output {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(
                output.status.success() && !text.contains("WARNING") && !text.contains("ERROR"),
                "qpdf rejected {}:\n{}",
                path.display(),
                text
            );
        }
    }

    doc
}

/// Rasterize the first page of `pdf` to `<prefix>.png` with `pdftoppm`.
/// Returns `false` when poppler is unavailable or rendering fails.
fn render_first_page(pdf: &Path, prefix: &Path) -> bool {
    let output = Command::new("pdftoppm")
        .args(["-singlefile", "-png", "-r", "72", "-f", "1", "-l", "1"])
        .arg(pdf)
        .arg(prefix)
        .output();
    matches!(output, Ok(out) if out.status.success())
}

/// Structural similarity (mean SSIM over the luminance channel, 64×64).
fn ssim(a: &GrayImage, b: &GrayImage) -> f64 {
    const N: u32 = 64;
    let a = image::imageops::resize(a, N, N, image::imageops::FilterType::Triangle);
    let b = image::imageops::resize(b, N, N, image::imageops::FilterType::Triangle);
    let count = (N * N) as f64;

    let mean = |img: &GrayImage| img.pixels().map(|p| p[0] as f64).sum::<f64>() / count;
    let (ma, mb) = (mean(&a), mean(&b));
    let var = |img: &GrayImage, m: f64| {
        img.pixels().map(|p| (p[0] as f64 - m).powi(2)).sum::<f64>() / count
    };
    let (va, vb) = (var(&a, ma), var(&b, mb));
    let cov = a
        .pixels()
        .zip(b.pixels())
        .map(|(x, y)| (x[0] as f64 - ma) * (y[0] as f64 - mb))
        .sum::<f64>()
        / count;

    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);
    ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2))
}

/// Rasterize pre/post versions of `name` and assert the pages are visually
/// equivalent (SSIM above `threshold`). Returns early when poppler is absent.
fn assert_visual_similarity(dir: &Path, name: &str, threshold: f64) {
    if !ensure_tool("pdftoppm", pdftoppm_available()) {
        return;
    }
    let (pre_pdf, post_pdf) = (
        dir.join(format!("{name}-pre.pdf")),
        dir.join(format!("{name}-post.pdf")),
    );
    let (pre_png, post_png) = (
        dir.join(format!("{name}-pre")),
        dir.join(format!("{name}-post")),
    );

    assert!(
        render_first_page(&pre_pdf, &pre_png),
        "pdftoppm failed on {name}-pre.pdf"
    );
    assert!(
        render_first_page(&post_pdf, &post_png),
        "pdftoppm failed on {name}-post.pdf"
    );

    let pre = image::open(pre_png.with_extension("png")).expect("pre render must decode");
    let post = image::open(post_png.with_extension("png")).expect("post render must decode");
    let score = ssim(&pre.to_luma8(), &post.to_luma8());
    assert!(
        score >= threshold,
        "SSIM {score:.4} below threshold {threshold} for {name}"
    );
}

fn decompressed_non_image_streams(doc: &Document) -> Vec<Vec<u8>> {
    doc.objects
        .values()
        .filter_map(|obj| match obj {
            // Uncompressed streams have no /Filter and fail `decompressed_content`,
            // so fall back to the raw content for those.
            Object::Stream(s) if s.dict.get(b"Subtype").is_err() => Some(
                s.decompressed_content()
                    .unwrap_or_else(|_| s.content.clone()),
            ),
            _ => None,
        })
        .collect()
}

fn count_image_streams(doc: &Document) -> usize {
    doc.objects
        .values()
        .filter(|obj| matches!(obj, Object::Stream(s) if s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok() == Some(b"Image".as_slice())))
        .count()
}

fn find_image_streams(doc: &Document) -> Vec<(lopdf::ObjectId, Vec<u8>, lopdf::Dictionary)> {
    doc.objects
        .iter()
        .filter_map(|(id, obj)| match obj {
            Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok()
                    == Some(b"Image".as_slice()) =>
            {
                Some((*id, s.content.clone(), s.dict.clone()))
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Multi-image PDF: structurally valid after compression, every image
/// re-encoded as DCTDecode, pages visually equivalent.
#[test]
fn multi_image_roundtrip_is_well_formed_and_reencoded() {
    let dir = test_dir();

    let images: Vec<(Vec<u8>, u32, u32, bool)> = (0..12)
        .map(|i| {
            let gray = i % 4 == 0; // every 4th image is grayscale
            let (w, h) = (128, 128 + (i % 3) * 16);
            let pixels = if gray {
                gradient_gray(w, h)
            } else {
                photoish_rgb(w, h)
            };
            (pixels, w, h, gray)
        })
        .collect();

    let mut pre = new_doc();
    for (pixels, w, h, gray) in &images {
        add_image_page(&mut pre.0, pre.1, pixels.clone(), *w, *h, *gray);
    }
    save_pdf(&mut pre.0, dir.join("multi-pre.pdf").to_str().unwrap()).unwrap();

    let mut post = new_doc();
    for (pixels, w, h, gray) in &images {
        add_image_page(&mut post.0, post.1, pixels.clone(), *w, *h, *gray);
    }
    compress_images(&mut post.0, QUALITY, false);
    compress_and_save_pdf(
        &mut post.0,
        dir.join("multi-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("multi-post.pdf"));
    assert_eq!(loaded.get_pages().len(), 12, "all pages must survive");

    let mut image_count = 0;
    for obj in loaded.objects.values() {
        if let Object::Stream(s) = obj
            && s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok() == Some(b"Image".as_slice())
        {
            image_count += 1;
            assert_eq!(
                s.dict.get(b"Filter").and_then(|f| f.as_name()).ok(),
                Some(b"DCTDecode".as_slice()),
                "re-encodable image must end up as DCTDecode"
            );
            assert_eq!(
                s.dict.get(b"Length").and_then(|l| l.as_i64()).ok(),
                Some(s.content.len() as i64)
            );
        }
    }
    assert_eq!(image_count, 12, "all 12 images must be re-encoded");

    assert_visual_similarity(&dir, "multi", 0.90);
}

/// Zero-image (text-only) PDF: nothing to re-encode, content untouched,
/// still renders identically.
#[test]
fn text_only_pdf_is_untouched() {
    let dir = test_dir();
    let (mut doc, pages_id) = new_doc();
    add_text_page(&mut doc, pages_id, 1);
    add_text_page(&mut doc, pages_id, 2);

    let content_before = decompressed_non_image_streams(&doc);
    compress_images(&mut doc, QUALITY, false);
    assert_eq!(
        decompressed_non_image_streams(&doc),
        content_before,
        "content streams must be untouched"
    );

    save_pdf(&mut doc, dir.join("text-pre.pdf").to_str().unwrap()).unwrap();
    compress_and_save_pdf(&mut doc, dir.join("text-post.pdf").to_str().unwrap(), false).unwrap();

    let loaded = assert_well_formed(&dir.join("text-post.pdf"));
    assert_eq!(loaded.get_pages().len(), 2);
    assert_eq!(
        count_image_streams(&loaded),
        0,
        "text-only PDF must stay image-free"
    );

    assert_visual_similarity(&dir, "text", 0.99);
}

/// CMYK streams are not supported by the image crate — they must be skipped
/// and preserved byte-for-byte.
#[test]
fn cmyk_stream_is_preserved() {
    let mut doc = cmyk_doc();
    let before = find_image_streams(&doc);
    assert_eq!(before.len(), 1);

    compress_images(&mut doc, QUALITY, false);

    let after = find_image_streams(&doc);
    assert_eq!(after.len(), 1, "CMYK stream must still be present");
    assert_eq!(
        after[0].1, before[0].1,
        "CMYK payload must be preserved byte-for-byte"
    );
    assert_eq!(after[0].2, before[0].2, "CMYK dict must be preserved");

    // Structural check end-to-end as well.
    let dir = test_dir();
    compress_and_save_pdf(&mut doc, dir.join("cmyk-post.pdf").to_str().unwrap(), false).unwrap();
    assert_well_formed(&dir.join("cmyk-post.pdf"));
}

/// Inline images live inside content streams and must never be touched by the
/// XObject image pass.
#[test]
fn inline_image_is_untouched() {
    let dir = test_dir();
    let mut doc = inline_image_doc();

    let content_before = decompressed_non_image_streams(&doc);
    compress_images(&mut doc, QUALITY, false);
    assert_eq!(
        decompressed_non_image_streams(&doc),
        content_before,
        "inline image content must be untouched"
    );

    save_pdf(&mut doc, dir.join("inline-pre.pdf").to_str().unwrap()).unwrap();
    compress_and_save_pdf(
        &mut doc,
        dir.join("inline-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("inline-post.pdf"));
    assert_eq!(
        count_image_streams(&loaded),
        0,
        "no XObject image streams expected"
    );

    let rendered = decompressed_non_image_streams(&loaded);
    assert!(
        rendered.iter().any(|c| c
            .windows(6)
            .any(|w| w == [0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00])),
        "inline pixel payload must still be present after round-trip"
    );

    assert_visual_similarity(&dir, "inline", 0.99);
}

/// Mixed FlateDecode streams: the image becomes DCTDecode, the text/metadata
/// streams keep their content.
#[test]
fn mixed_flate_streams_are_handled() {
    let dir = test_dir();
    let mut doc = mixed_flate_doc();
    save_pdf(&mut doc, dir.join("mixed-pre.pdf").to_str().unwrap()).unwrap();

    let metadata_before = doc
        .objects
        .values()
        .filter_map(|obj| match obj {
            Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok()
                    == Some(b"XML".as_slice()) =>
            {
                s.decompressed_content().ok()
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    compress_images(&mut doc, QUALITY, false);

    let metadata_after = doc
        .objects
        .values()
        .filter_map(|obj| match obj {
            Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok()
                    == Some(b"XML".as_slice()) =>
            {
                s.decompressed_content().ok()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        metadata_after, metadata_before,
        "metadata stream must be preserved"
    );

    compress_and_save_pdf(
        &mut doc,
        dir.join("mixed-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();
    let loaded = assert_well_formed(&dir.join("mixed-post.pdf"));
    assert_eq!(count_image_streams(&loaded), 1);
    assert_visual_similarity(&dir, "mixed", 0.90);
}

/// A DeviceRGB stream with a 4th (alpha) byte per pixel is normalized by the
/// chunked path and re-encoded without visual drift.
#[test]
fn rgba4_stream_is_normalized_in_chunks() {
    let dir = test_dir();
    // Ground truth: the canonical 3-byte/px interpretation of the same pixels.
    let mut pre = rgb3_doc(128, 96);
    save_pdf(&mut pre, dir.join("rgba-pre.pdf").to_str().unwrap()).unwrap();

    let mut post = rgba4_doc(128, 96);
    compress_images(&mut post, QUALITY, false);
    compress_and_save_pdf(
        &mut post,
        dir.join("rgba-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("rgba-post.pdf"));
    let mut reencoded = 0;
    for obj in loaded.objects.values() {
        if let Object::Stream(s) = obj
            && s.dict.get(b"Subtype").and_then(|x| x.as_name()).ok() == Some(b"Image".as_slice())
        {
            reencoded += 1;
            assert_eq!(
                s.dict.get(b"Filter").and_then(|f| f.as_name()).ok(),
                Some(b"DCTDecode".as_slice())
            );
            assert_eq!(
                s.dict.get(b"Width").and_then(|w| w.as_i64()).ok(),
                Some(128)
            );
            assert_eq!(
                s.dict.get(b"Height").and_then(|h| h.as_i64()).ok(),
                Some(96)
            );
        }
    }
    assert_eq!(
        reencoded, 1,
        "4-byte/px stream must be normalized and re-encoded"
    );

    assert_visual_similarity(&dir, "rgba", 0.95);
}

/// Serial (1-thread pool) and parallel (global pool) runs must produce
/// byte-identical documents — parallelism must not change the output.
#[test]
fn parallel_and_serial_outputs_are_identical() {
    let dir = test_dir();
    let build = || {
        let (mut doc, pages_id) = new_doc();
        for _ in 0..8 {
            add_image_page(&mut doc, pages_id, photoish_rgb(96, 96), 96, 96, false);
        }
        doc
    };

    let mut serial = build();
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| compress_images(&mut serial, QUALITY, false));

    let mut parallel = build();
    compress_images(&mut parallel, QUALITY, false);

    let (serial_path, parallel_path) = (
        dir.join("determinism-serial.pdf"),
        dir.join("determinism-parallel.pdf"),
    );
    compress_and_save_pdf(&mut serial, serial_path.to_str().unwrap(), false).unwrap();
    compress_and_save_pdf(&mut parallel, parallel_path.to_str().unwrap(), false).unwrap();

    assert_eq!(
        std::fs::read(&serial_path).unwrap(),
        std::fs::read(&parallel_path).unwrap(),
        "serial and parallel compression must produce identical bytes"
    );
}

/// End-to-end: the actual `presse press` CLI binary produces a loadable,
/// renderable, smaller PDF.
#[test]
fn press_cli_end_to_end() {
    let dir = test_dir();
    let (mut doc, pages_id) = new_doc();
    for _ in 0..6 {
        add_image_page(&mut doc, pages_id, photoish_rgb(160, 160), 160, 160, false);
    }
    save_pdf(&mut doc, dir.join("cli-pre.pdf").to_str().unwrap()).unwrap();

    let presse = env!("CARGO_BIN_EXE_presse");
    let out = dir.join("cli-post.pdf");
    let status = Command::new(presse)
        .args([
            "press",
            dir.join("cli-pre.pdf").to_str().unwrap(),
            "-q",
            "50",
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("presse binary should run");
    assert!(status.success(), "presse press must exit successfully");

    assert_well_formed(&out);
    assert!(
        std::fs::metadata(&out).unwrap().len()
            < std::fs::metadata(dir.join("cli-pre.pdf")).unwrap().len(),
        "compressed output must be smaller"
    );
    assert_visual_similarity(&dir, "cli", 0.90);
}

/// Real-world PDFs carry gaps in their object numbering and usually exceed
/// one object stream (>200 objects). Both conditions used to corrupt lopdf's
/// cross-reference writer — shifted sections at every gap plus dropped
/// object-stream entries — which qpdf rejects and poppler can render as
/// blank pages. This test builds a document with both properties and asserts
/// the compressed output stays valid (qpdf gate + render).
///
/// Passes on lopdf >= 0.44 because the save path already calls
/// `renumber_objects()`: compacting the numbering makes the writer's
/// remaining gap/size quirks unreachable (verified: same /Size, same entry
/// count, nothing missing — identical to the previously vendored writer
/// patch).
#[test]
fn gapped_numbering_with_multiple_object_streams_is_valid() {
    let dir = test_dir();

    let build = || {
        let (mut doc, pages_id) = new_doc();
        // Enough pages to span multiple object streams on save (>200 objects).
        for page in 0..90 {
            add_text_page(&mut doc, pages_id, page);
        }
        // An orphaned object that nothing references, then delete it: creates
        // a gap in the object numbering without touching the page tree.
        let orphan = doc.add_object(Object::String(
            b"orphan".to_vec(),
            lopdf::StringFormat::Literal,
        ));
        doc.delete_object(orphan);
        doc
    };

    let mut pre = build();
    save_pdf(&mut pre, dir.join("gapped-pre.pdf").to_str().unwrap()).unwrap();

    let mut post = build();
    compress_and_save_pdf(
        &mut post,
        dir.join("gapped-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("gapped-post.pdf"));
    assert_eq!(loaded.get_pages().len(), 90, "all pages must survive");
    assert_visual_similarity(&dir, "gapped", 0.99);
}

/// `--dpi` downsampling: a 300×300 image drawn at 100×100 pt is 216 dpi
/// effective; at `-d 150` it must be resampled to ≈100·150/72 = 208 px and
/// the `/Width`/`/Height` dictionary entries updated to match, with the
/// output still structurally valid and visually recognizable.
#[test]
fn dpi_downsampling_resizes_placed_images_and_updates_dims() {
    let dir = test_dir();

    let mut pre = new_doc();
    add_image_page_at(
        &mut pre.0,
        pre.1,
        photoish_rgb(300, 300),
        300,
        300,
        false,
        (100.0, 100.0),
    );
    save_pdf(&mut pre.0, dir.join("dpi-pre.pdf").to_str().unwrap()).unwrap();

    let mut post = new_doc();
    add_image_page_at(
        &mut post.0,
        post.1,
        photoish_rgb(300, 300),
        300,
        300,
        false,
        (100.0, 100.0),
    );
    compress_images_with(
        &mut post.0,
        QualityMode::fixed(QUALITY),
        false,
        &CpuTranscoder,
        Some(150),
    );
    compress_and_save_pdf(
        &mut post.0,
        dir.join("dpi-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("dpi-post.pdf"));
    let images = find_image_streams(&loaded);
    assert_eq!(images.len(), 1);
    let (_, _, dict) = &images[0];
    assert_eq!(
        dict.get(b"Width").and_then(|w| w.as_i64()).ok(),
        Some(208),
        "/Width must reflect the resampled raster"
    );
    assert_eq!(
        dict.get(b"Height").and_then(|h| h.as_i64()).ok(),
        Some(208),
        "/Height must reflect the resampled raster"
    );
    assert_eq!(
        dict.get(b"Filter").and_then(|f| f.as_name()).ok(),
        Some(b"DCTDecode".as_slice()),
        "resampled image must be re-encoded as DCTDecode"
    );

    // Same placement, less detail: still the same page at high SSIM.
    assert_visual_similarity(&dir, "dpi", 0.85);
}

/// `--dpi` above the effective resolution must not bite: a 300×300 image at
/// 216 dpi effective stays 300×300 under a 600 dpi cap.
#[test]
fn dpi_above_effective_resolution_keeps_source_size() {
    let dir = test_dir();

    let mut doc = new_doc();
    add_image_page_at(
        &mut doc.0,
        doc.1,
        photoish_rgb(300, 300),
        300,
        300,
        false,
        (100.0, 100.0),
    );
    compress_images_with(
        &mut doc.0,
        QualityMode::fixed(QUALITY),
        false,
        &CpuTranscoder,
        Some(600),
    );
    compress_and_save_pdf(
        &mut doc.0,
        dir.join("dpi-noop.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("dpi-noop.pdf"));
    let images = find_image_streams(&loaded);
    assert_eq!(images.len(), 1);
    let (_, _, dict) = &images[0];
    assert_eq!(dict.get(b"Width").and_then(|w| w.as_i64()).ok(), Some(300));
    assert_eq!(dict.get(b"Height").and_then(|h| h.as_i64()).ok(), Some(300));
}

/// CLI end-to-end: `presse press -d 75` on a high-resolution placed image
/// produces a valid, smaller PDF (qpdf + gs + xref gates in
/// `assert_well_formed`) whose image raster was actually reduced.
#[test]
fn press_cli_dpi_flag_resamples_and_stays_valid() {
    let dir = test_dir();
    let mut doc = new_doc();
    add_image_page_at(
        &mut doc.0,
        doc.1,
        photoish_rgb(600, 600),
        600,
        600,
        false,
        (200.0, 200.0),
    );
    save_pdf(&mut doc.0, dir.join("dpi-cli-pre.pdf").to_str().unwrap()).unwrap();

    let presse = env!("CARGO_BIN_EXE_presse");
    let out = dir.join("dpi-cli-post.pdf");
    let status = Command::new(presse)
        .args([
            "press",
            dir.join("dpi-cli-pre.pdf").to_str().unwrap(),
            "-q",
            "50",
            "-d",
            "75",
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("presse binary should run");
    assert!(
        status.success(),
        "presse press -d 75 must exit successfully"
    );

    let loaded = assert_well_formed(&out);
    let images = find_image_streams(&loaded);
    assert_eq!(images.len(), 1);
    let (_, _, dict) = &images[0];
    // 600 px at 200 pt = 216 dpi effective → 200·75/72 ≈ 208 px.
    assert_eq!(dict.get(b"Width").and_then(|w| w.as_i64()).ok(), Some(208));
    assert_eq!(dict.get(b"Height").and_then(|h| h.as_i64()).ok(), Some(208));
    assert_visual_similarity(&dir, "dpi-cli", 0.85);
}

// ---------------------------------------------------------------------------
// Pluggable acceleration (`--acceleration`) — all testable without a GPU.
// ---------------------------------------------------------------------------

/// A GPU backend that always fails, standing in for "driver disabled at
/// runtime". Proves [`FallbackTranscoder`] degrades to CPU without dropping
/// or corrupting a stream (acceptance criterion 3).
struct FailingGpu;

impl ImageTranscoder for FailingGpu {
    fn transcode_image(&self, _input: &Input, _quality: u8) -> Result<Vec<u8>, TranscodeError> {
        Err(TranscodeError::Gpu("simulated driver failure".into()))
    }
}

/// A GPU backend that records how often it is consulted.
struct RecordingGpu(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl ImageTranscoder for RecordingGpu {
    fn transcode_image(&self, _input: &Input, _quality: u8) -> Result<Vec<u8>, TranscodeError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Vec::new())
    }
}

/// With the driver "disabled", the fallback transcoder must produce output
/// byte-identical to the pure-CPU path, and that output must pass the full
/// structural + visual gates.
#[test]
fn gpu_failure_falls_back_to_cpu_identically() {
    let dir = test_dir();
    let build = || {
        let (mut doc, pages_id) = new_doc();
        for _ in 0..8 {
            add_image_page(&mut doc, pages_id, photoish_rgb(96, 96), 96, 96, false);
        }
        doc
    };

    let mut cpu_doc = build();
    compress_images(&mut cpu_doc, QUALITY, false);

    // threshold 0 → every stream consults the (failing) GPU first.
    let fallback = FallbackTranscoder::new(Some(FailingGpu), 0);
    let mut fb_doc = build();
    compress_images_with(
        &mut fb_doc,
        QualityMode::fixed(QUALITY),
        false,
        &fallback,
        None,
    );

    let (cpu_path, fb_path) = (
        dir.join("gpu-fallback-pre.pdf"),
        dir.join("gpu-fallback-post.pdf"),
    );
    compress_and_save_pdf(&mut cpu_doc, cpu_path.to_str().unwrap(), false).unwrap();
    compress_and_save_pdf(&mut fb_doc, fb_path.to_str().unwrap(), false).unwrap();

    assert_eq!(
        std::fs::read(&cpu_path).unwrap(),
        std::fs::read(&fb_path).unwrap(),
        "fallback output must be byte-identical to the CPU backend"
    );
    assert_well_formed(&fb_path);
    assert_visual_similarity(&dir, "gpu-fallback", 0.99);
}

/// Streams below the PCIe-latency threshold must never reach the GPU;
/// larger streams must. (No GPU is involved — the recording backend
/// simulates one.)
#[test]
fn gpu_routing_respects_stream_size_threshold() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fallback = FallbackTranscoder::new(
        Some(RecordingGpu(std::sync::Arc::clone(&calls))),
        128 * 1024,
    );

    // Below the threshold: CPU only (decode of garbage fails, but that is
    // irrelevant — the GPU must not have been consulted).
    let small = Input::Jpeg(&[0u8; 1024]);
    assert!(fallback.transcode_image(&small, QUALITY).is_err());
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "small streams stay on CPU"
    );

    // Above the threshold: GPU first.
    let large = Input::Jpeg(&vec![0u8; 200_000]);
    let buf = fallback.transcode_image(&large, QUALITY).unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "large streams reach the GPU"
    );
    assert!(buf.is_empty());
}

/// Requesting a backend that was not compiled in must fail with an explicit
/// error naming the missing Cargo flag — never a crash (requirement 1.3).
#[test]
fn unbuilt_acceleration_is_an_explicit_error() {
    #[cfg(not(feature = "cuda"))]
    {
        let err = resolve(Acceleration::Cuda).unwrap_err();
        assert!(
            err.contains("--features cuda"),
            "error must name the missing flag: {err}"
        );
    }
    #[cfg(not(feature = "rocm"))]
    {
        let err = resolve(Acceleration::Rocm).unwrap_err();
        assert!(
            err.contains("--features rocm"),
            "error must name the missing flag: {err}"
        );
    }
    // cpu always resolves; auto resolves to cpu without a driver.
    assert!(matches!(
        resolve(Acceleration::Cpu),
        Ok(RuntimeTranscoder::Cpu(_))
    ));
    #[cfg(not(any(feature = "cuda", feature = "rocm")))]
    assert!(matches!(
        resolve(Acceleration::Auto),
        Ok(RuntimeTranscoder::Cpu(_))
    ));
}

/// The CLI must reject `--acceleration cuda` on a build without the feature
/// with a clear error, and accept the default `cpu` path.
#[test]
fn press_cli_acceleration_flag() {
    let dir = test_dir();
    let (mut doc, pages_id) = new_doc();
    for _ in 0..3 {
        add_image_page(&mut doc, pages_id, photoish_rgb(96, 96), 96, 96, false);
    }
    save_pdf(&mut doc, dir.join("acc-in.pdf").to_str().unwrap()).unwrap();
    let presse = env!("CARGO_BIN_EXE_presse");

    // Default CPU path with the explicit flag.
    let ok = Command::new(presse)
        .args([
            "press",
            dir.join("acc-in.pdf").to_str().unwrap(),
            "-a",
            "cpu",
            "-o",
            dir.join("acc-cpu.pdf").to_str().unwrap(),
        ])
        .status()
        .expect("presse binary should run");
    assert!(ok.success(), "-a cpu must succeed");
    assert_well_formed(&dir.join("acc-cpu.pdf"));

    // Unbuilt backend → explicit error.
    #[cfg(not(feature = "cuda"))]
    {
        let run = Command::new(presse)
            .args([
                "press",
                dir.join("acc-in.pdf").to_str().unwrap(),
                "-a",
                "cuda",
                "-o",
                dir.join("acc-cuda.pdf").to_str().unwrap(),
            ])
            .output()
            .expect("presse binary should run");
        assert!(
            !run.status.success(),
            "-a cuda must fail without the feature"
        );
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            stderr.contains("--features cuda"),
            "stderr must name the missing flag: {stderr}"
        );
    }
}

/// The CPU transcoder is the zero-cost default: `compress_images_with` over
/// [`CpuTranscoder`] must equal the plain `compress_images` path.
#[test]
fn cpu_transcoder_default_parity() {
    let dir = test_dir();
    let build = || {
        let (mut doc, pages_id) = new_doc();
        for i in 0..6 {
            let gray = i % 3 == 0;
            let (w, h) = (96, 96);
            let pixels = if gray {
                gradient_gray(w, h)
            } else {
                photoish_rgb(w, h)
            };
            add_image_page(&mut doc, pages_id, pixels, w, h, gray);
        }
        doc
    };

    let mut plain = build();
    compress_images(&mut plain, QUALITY, false);
    let mut explicit = build();
    compress_images_with(
        &mut explicit,
        QualityMode::fixed(QUALITY),
        false,
        &CpuTranscoder,
        None,
    );

    let (p_path, e_path) = (
        dir.join("parity-plain.pdf"),
        dir.join("parity-explicit.pdf"),
    );
    compress_and_save_pdf(&mut plain, p_path.to_str().unwrap(), false).unwrap();
    compress_and_save_pdf(&mut explicit, e_path.to_str().unwrap(), false).unwrap();
    assert_eq!(
        std::fs::read(&p_path).unwrap(),
        std::fs::read(&e_path).unwrap(),
        "CpuTranscoder must match the default path byte-for-byte"
    );
}

/// `-d` is a resolution *cap*: output dimensions must never exceed the
/// source, and must match ⌊placed·dpi/72⌋ when that bites. Property-style
/// over a few placements so the "never up-scales" rule is pinned.
#[test]
fn dpi_cap_never_upscales_and_matches_formula() {
    let dir = test_dir();
    let cases = [
        // (source W, source H, placed W, placed H, dpi, expected W, expected H)
        (400, 300, 100.0, 100.0, 150, 208, 208), // 216 dpi effective → 150 cap
        (400, 300, 100.0, 100.0, 300, 400, 300), // 416 > source → capped at source
        (100, 100, 400.0, 400.0, 75, 100, 100),  // placed low-dpi → no change
        (512, 256, 200.0, 100.0, 600, 512, 256), // 1667/833 > source → capped
    ];
    for (i, (w, h, pw, ph, dpi, ew, eh)) in cases.iter().enumerate() {
        let mut doc = new_doc();
        add_image_page_at(
            &mut doc.0,
            doc.1,
            photoish_rgb(*w, *h),
            *w,
            *h,
            false,
            (*pw, *ph),
        );
        compress_images_with(
            &mut doc.0,
            QualityMode::fixed(QUALITY),
            false,
            &CpuTranscoder,
            Some(*dpi),
        );
        compress_and_save_pdf(
            &mut doc.0,
            dir.join(format!("dpi-prop-{i}.pdf")).to_str().unwrap(),
            false,
        )
        .unwrap();
        let loaded = assert_well_formed(&dir.join(format!("dpi-prop-{i}.pdf")));
        let images = find_image_streams(&loaded);
        assert_eq!(images.len(), 1);
        let (_, _, dict) = &images[0];
        let ow = dict.get(b"Width").and_then(|x| x.as_i64()).ok().unwrap() as u32;
        let oh = dict.get(b"Height").and_then(|x| x.as_i64()).ok().unwrap() as u32;
        assert!(
            ow <= *w && oh <= *h,
            "case {i}: cap must never up-scale ({ow}x{oh} > {w}x{h})"
        );
        let formula = |placed: f64| ((placed * *dpi as f64 / 72.0).round() as u32).max(1);
        assert!(
            ow <= formula(*pw) && oh <= formula(*ph),
            "case {i}: output {ow}x{oh} exceeds ⌊placed·dpi/72⌋ {}x{}",
            formula(*pw),
            formula(*ph)
        );
        assert_eq!((ow, oh), (*ew, *eh), "case {i}: expected dims");
    }
}

/// After the parallel rewrite, every object in the saved document must be
/// reachable from the trailer's /Root (no orphans from the detach/re-attach
/// phase), and the file must load with a valid cross-reference table.
#[test]
fn xref_is_well_formed_and_all_objects_reachable() {
    let dir = test_dir();

    let (mut doc, pages_id) = new_doc();
    for _ in 0..8 {
        add_image_page(&mut doc, pages_id, photoish_rgb(160, 160), 160, 160, false);
    }
    compress_images_with(
        &mut doc,
        QualityMode::fixed(QUALITY),
        false,
        &CpuTranscoder,
        Some(150),
    );
    compress_and_save_pdf(
        &mut doc,
        dir.join("reach-post.pdf").to_str().unwrap(),
        false,
    )
    .unwrap();

    let loaded = assert_well_formed(&dir.join("reach-post.pdf"));

    // BFS from /Root through every reference; every object must be found.
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<lopdf::ObjectId> = loaded
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|r| r.as_reference().ok())
        .map(|id| vec![id])
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(obj) = loaded.get_object(id).ok() else {
            continue;
        };
        match obj {
            Object::Dictionary(d) => {
                for (_, v) in d.iter() {
                    collect_references(v, &mut stack);
                }
            }
            Object::Array(a) => {
                for v in a {
                    collect_references(v, &mut stack);
                }
            }
            Object::Stream(s) => {
                for (_, v) in s.dict.iter() {
                    collect_references(v, &mut stack);
                }
            }
            _ => {}
        }
    }
    let all: std::collections::HashSet<lopdf::ObjectId> = loaded.objects.keys().copied().collect();
    // Structural objects (object-stream container, xref stream) are kept in
    // `doc.objects` by the loader but are not part of the content graph.
    let structural = |obj: &Object| {
        matches!(obj, Object::Stream(s)
            if s.dict.get(b"Type").ok().and_then(|t| t.as_name().ok())
                .is_some_and(|t| t == b"ObjStm" || t == b"XRef"))
    };
    let content_ids: Vec<_> = all
        .iter()
        .copied()
        .filter(|id| !loaded.objects.get(id).is_some_and(structural))
        .collect();
    let missing: Vec<_> = content_ids
        .iter()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "content objects not reachable from /Root: {missing:?}"
    );
    assert_eq!(
        seen.len(),
        content_ids.len(),
        "reachable set must equal content-object set"
    );
}

/// Push every reference inside `obj` (dictionary values, array elements)
/// onto `stack`.
fn collect_references(obj: &Object, stack: &mut Vec<lopdf::ObjectId>) {
    match obj {
        Object::Reference(id) => stack.push(*id),
        Object::Dictionary(d) => {
            for (_, v) in d.iter() {
                collect_references(v, stack);
            }
        }
        Object::Array(a) => {
            for v in a {
                collect_references(v, stack);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dict.iter() {
                collect_references(v, stack);
            }
        }
        _ => {}
    }
}

/// A grayscale source must stay a single-component JPEG after re-encoding —
/// a 3-component JPEG inside a /DeviceGray stream renders as garbage.
#[test]
fn grayscale_jpeg_stays_single_component() {
    let dir = test_dir();

    let (mut doc, pages_id) = new_doc();
    add_image_page(&mut doc, pages_id, gradient_gray(128, 128), 128, 128, true);
    compress_images_with(
        &mut doc,
        QualityMode::fixed(QUALITY),
        false,
        &CpuTranscoder,
        None,
    );
    compress_and_save_pdf(&mut doc, dir.join("gray-post.pdf").to_str().unwrap(), false).unwrap();

    let loaded = assert_well_formed(&dir.join("gray-post.pdf"));
    let images = find_image_streams(&loaded);
    assert_eq!(images.len(), 1);
    let (_, content, dict) = &images[0];
    assert_eq!(
        dict.get(b"Filter").and_then(|f| f.as_name()).ok(),
        Some(b"DCTDecode".as_slice()),
        "grayscale image must be re-encoded as DCTDecode"
    );
    let img = image::load_from_memory(content).expect("output JPEG must decode");
    assert_eq!(
        img.color(),
        image::ColorType::L8,
        "single-component grayscale must survive re-encoding"
    );
}

/// The rayon + allocator path must be safe under many concurrent
/// compression jobs on the same binary (global thread pool + mimalloc),
/// with every output identical and valid.
#[test]
fn concurrent_compression_is_deterministic_and_valid() {
    let dir = test_dir();
    let build = || {
        let (mut doc, pages_id) = new_doc();
        for _ in 0..6 {
            add_image_page(&mut doc, pages_id, photoish_rgb(160, 160), 160, 160, false);
        }
        doc
    };

    let outputs: Vec<PathBuf> = (0..8)
        .map(|i| {
            let path = dir.join(format!("conc-{i}.pdf"));
            let mut doc = build();
            compress_images_with(
                &mut doc,
                QualityMode::fixed(QUALITY),
                false,
                &CpuTranscoder,
                None,
            );
            compress_and_save_pdf(&mut doc, path.to_str().unwrap(), false).unwrap();
            path
        })
        .collect();

    let first = std::fs::read(&outputs[0]).unwrap();
    for path in &outputs {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes, first, "concurrent runs must be byte-identical");
        assert_well_formed(path);
    }
}

/// `-ssim <target>` maps to a lower JPEG quality than the default (smaller
/// output), stays structurally valid, and renders recognizably. The
/// calibration guarantees the target on grainy content; photos exceed it,
/// so the visual gate is lenient by design.
#[test]
fn ssim_target_reduces_size_and_stays_valid() {
    let dir = test_dir();

    let build = |mode: QualityMode, name: &str| {
        let mut doc = new_doc();
        for _ in 0..4 {
            add_image_page(&mut doc.0, doc.1, gradient_gray(256, 256), 256, 256, true);
        }
        compress_images_with(&mut doc.0, mode, false, &CpuTranscoder, None);
        compress_and_save_pdf(&mut doc.0, dir.join(name).to_str().unwrap(), false).unwrap();
        std::fs::metadata(dir.join(name)).unwrap().len()
    };

    let default_size = build(QualityMode::fixed(QUALITY), "ssim-pre.pdf");
    let relaxed = build(QualityMode::press(QUALITY, Some(0.72)), "ssim-post.pdf");
    assert!(
        relaxed < default_size,
        "-ssim 0.72 must compress harder than the default: {relaxed} vs {default_size}"
    );

    let loaded = assert_well_formed(&dir.join("ssim-post.pdf"));
    assert_eq!(loaded.get_pages().len(), 4, "all pages must survive");
    assert_visual_similarity(&dir, "ssim", 0.85);
}

/// `-ssim` ≥ 1.0 is the default behavior: `-q` as given, byte-identical
/// to not passing the flag at all.
#[test]
fn ssim_one_keeps_default_quality() {
    let dir = test_dir();

    let run = |mode: QualityMode, name: &str| {
        let mut doc = new_doc();
        add_image_page(&mut doc.0, doc.1, photoish_rgb(200, 200), 200, 200, false);
        compress_images_with(&mut doc.0, mode, false, &CpuTranscoder, None);
        compress_and_save_pdf(&mut doc.0, dir.join(name).to_str().unwrap(), false).unwrap();
        std::fs::read(dir.join(name)).unwrap()
    };

    let plain = run(QualityMode::fixed(QUALITY), "ssim1-plain.pdf");
    let with_flag = run(QualityMode::press(QUALITY, Some(1.0)), "ssim1-flag.pdf");
    assert_eq!(
        plain, with_flag,
        "-ssim 1.0 must equal the plain -q behavior"
    );
}

/// The two quality knobs compose: `-d 150 -s 0.72` both resamples *and*
/// compresses harder — dimensions shrink to the dpi cap and the output is
/// smaller than either knob alone.
#[test]
fn dpi_and_ssim_compose() {
    let dir = test_dir();

    let run = |mode: QualityMode, dpi: Option<u32>, name: &str| -> (u64, u32, u32) {
        let mut doc = new_doc();
        // 600 px placed at 200 pt = 216 dpi effective.
        add_image_page_at(
            &mut doc.0,
            doc.1,
            photoish_rgb(600, 600),
            600,
            600,
            false,
            (200.0, 200.0),
        );
        compress_images_with(&mut doc.0, mode, false, &CpuTranscoder, dpi);
        compress_and_save_pdf(&mut doc.0, dir.join(name).to_str().unwrap(), false).unwrap();
        let loaded = Document::load(dir.join(name)).unwrap();
        let (_, _, dict) = &find_image_streams(&loaded)[0];
        let w = dict.get(b"Width").and_then(|x| x.as_i64()).ok().unwrap() as u32;
        let h = dict.get(b"Height").and_then(|x| x.as_i64()).ok().unwrap() as u32;
        (std::fs::metadata(dir.join(name)).unwrap().len(), w, h)
    };

    let (base_size, _, _) = run(QualityMode::fixed(QUALITY), None, "compose-base.pdf");
    let (d_size, dw, _) = run(QualityMode::fixed(QUALITY), Some(150), "compose-d.pdf");
    let (s_size, _, _) = run(
        QualityMode::press(QUALITY, Some(0.72)),
        None,
        "compose-s.pdf",
    );
    let (ds_size, dsw, _) = run(
        QualityMode::press(QUALITY, Some(0.72)),
        Some(150),
        "compose-ds.pdf",
    );

    assert_eq!(
        dw, 417,
        "-d 150 on a 216-dpi placement → round(200·150/72) = 417 px"
    );
    assert_eq!(dsw, 417, "dpi cap applies regardless of the ssim target");
    assert!(
        ds_size < d_size && ds_size < s_size && s_size < base_size,
        "composed dpi+ssim must beat each knob alone: base {base_size}, d {d_size}, s {s_size}, ds {ds_size}"
    );
    assert_well_formed(&dir.join("compose-ds.pdf"));
}
