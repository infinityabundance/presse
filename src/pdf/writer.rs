use lopdf::{Document, Object, SaveOptions};
use std::fs::File;

use crate::pdf::images::zlib_encode;

/// qpdf's structural trick (`--recompress-flate`): existing `FlateDecode`
/// streams are usually stored at a lower compression level than this writer
/// uses (form tools write ~level 6; we write level 9), so decoding and
/// re-encoding them shrinks the file without touching a single content
/// byte. Only pure-Flate streams (a `FlateDecode` name or a single-element
/// `[FlateDecode]` array) with no `/DecodeParms` are touched — DCT/LZW/
/// multi-filter chains and predictor-parameterized streams are left alone,
/// as is anything that fails to decompress. Returns the number of streams
/// recompressed.
///
/// Losslessness is structural: Flate decode + re-encode is bit-exact on
/// the decoded bytes, so pixels, text, metadata and fonts are unchanged;
/// only the compressed representation differs.
pub fn recompress_flate(doc: &mut Document) -> usize {
    let mut recompressed = 0;
    for (_, obj) in doc.objects.iter_mut() {
        let Object::Stream(stream) = obj else {
            continue;
        };
        // Pure FlateDecode only, and no predictor parameters: re-encoding
        // without them would change the decoded output.
        let is_flate = match stream.dict.get(b"Filter") {
            Ok(Object::Name(n)) => n == b"FlateDecode",
            Ok(Object::Array(a)) => {
                a.len() == 1 && a[0].as_name().is_ok_and(|n| n == b"FlateDecode")
            }
            _ => false,
        };
        if !is_flate || stream.dict.get(b"DecodeParms").is_ok() {
            continue;
        }
        let Ok(decoded) = stream.decompressed_content() else {
            continue;
        };
        let re = zlib_encode(&decoded);
        if re.len() < stream.content.len() {
            stream.content = re;
            stream
                .dict
                .set(b"Length", Object::Integer(stream.content.len() as i64));
            recompressed += 1;
        }
    }
    recompressed
}

pub fn compress_and_save_pdf(
    doc: &mut Document,
    name: &str,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let option: SaveOptions = SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .max_objects_per_stream(200)
        .compression_level(9)
        .build();

    verbose!(
        verbose,
        "[writer] {} objects before cleanup",
        doc.objects.len()
    );
    Document::delete_zero_length_streams(doc);
    verbose!(
        verbose,
        "[writer] {} objects after cleanup",
        doc.objects.len()
    );

    // Real-world PDFs usually carry gaps in their object numbering (deleted
    // objects, preserved source ids). lopdf's xref writer opens a new xref
    // section at the first *missing* id but appends the next present object's
    // entry to it, shifting every subsequent entry whenever a gap exists — a
    // corruption qpdf rejects and poppler can render as blank pages. Renumber
    // to contiguous ids first so both the xref-table and xref-stream writers
    // stay correct.
    doc.renumber_objects();

    doc.compress();

    verbose!(verbose, "[writer] saving to '{}'", name);
    let mut file = File::create(name)?;
    doc.save_with_options(&mut file, option)
        .map_err(|e| format!("save_with_options failed for '{}': {}", name, e))?;

    Ok(())
}

pub fn save_pdf(doc: &mut Document, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Document::delete_zero_length_streams(doc);
    // See compress_and_save_pdf: lopdf's xref writers mis-handle gaps in
    // object numbering, so compact the ids before saving.
    doc.renumber_objects();
    doc.compress();
    doc.save(name)?;
    Ok(())
}
