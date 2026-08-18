# Vendored `lopdf` 0.39.0 — patch notes

This directory is an exact copy of [`lopdf`](https://github.com/J-F-Liu/lopdf)
v0.39.0 (MIT license, see `LICENSE`, docs in `README.md`), wired into the
build via `[patch.crates-io]` in the workspace `Cargo.toml`.

## Why it is vendored

Upstream lopdf 0.39.0 (and current `master`) writes corrupt cross-reference
data in two situations that are common in real-world PDFs:

1. **Sections start at missing object ids.** Both `write_xref` and
   `create_xref_steam` open a new xref section at the first *missing* object
   id and then append the next *present* object's entry to it. Every gap in
   the object numbering therefore shifts every subsequent entry by one.
2. **Entries past the pre-stream `size` are dropped.** `xref.size` is fixed
   before object streams and the xref stream itself are appended, and both
   writers iterate `1..size`, silently omitting the object-stream and xref
   entries. Any document with more than ~200 objects (i.e. essentially any
   real PDF) loses its trailing object streams from the cross-reference.

The result is a technically-invalid cross-reference. `qpdf --check` reports
`supposed object stream N is not a stream` / `unable to find page tree`, and
poppler can render pages as **blank** (observed on the PDF 1.7 specification,
a 756-page, 127k-object document). Ghostscript and lopdf's own reader are
lenient enough to recover, which is why this went unnoticed.

## The patch

`src/writer.rs` — two surgical changes, mirrored in both writers:

- Iterate `1..=xref.max_id()` instead of `1..xref.size` so every inserted
  entry (object streams, xref stream) is covered.
- Start each section lazily at the first *present* id (the
  `if section.is_empty() { section = XrefSection::new(obj_id) }` guard lives
  inside the present-entry branch), and reset the section to a zero
  placeholder when a gap closes it.

No public API changes; the crate is otherwise byte-for-byte upstream 0.39.0.
The only other deviation: three unit tests that `include_bytes!`/`read_dir`
from `assets/` were removed (the assets directory is not vendored; the tests
are `#[cfg(test)]`-only and never compiled in normal builds).

## Keeping it in sync

To refresh from upstream, re-copy the crate and re-apply the `src/writer.rs`
change described above. Consider upstreaming the fix.
