use lopdf::{Document, SaveOptions};
use std::fs::File;

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
