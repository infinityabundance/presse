#!/usr/bin/env python3
"""Apply the PRESSE_REQUIRE_PDF_TOOLS gate edits to tests/regression.rs
deterministically (bypassing the editor's stale file cache)."""
import pathlib
import sys

p = pathlib.Path("tests/regression.rs")
s = p.read_text()

# 1. Insert helper block after the qpdf_available() fn (assert it's present once).
helper = '''/// True when qpdf is installed.
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
'''
assert s.count("fn qpdf_available() -> bool {") == 1, "qpdf_available should appear once"
assert "fn ensure_tool" not in s, "ensure_tool should not exist yet"
s = s.replace(
    '''/// True when qpdf is installed.
fn qpdf_available() -> bool {
    Command::new("qpdf").arg("--version").output().is_ok()
}
''',
    helper,
    1,
)

# 2. gs gate
assert s.count("if gs_available() {") == 1, "one gs gate expected"
s = s.replace("if gs_available() {", 'if ensure_tool("gs", gs_available()) {', 1)

# 3. qpdf gate
assert s.count("if qpdf_available() {") == 1, "one qpdf gate expected"
s = s.replace("if qpdf_available() {", 'if ensure_tool("qpdf", qpdf_available()) {', 1)

# 4. visual similarity gate
old_vis = '''    if !pdftoppm_available() {
        eprintln!("note: pdftoppm not found — skipping visual check for {name}");
        return;
    }'''
new_vis = '''    if !ensure_tool("pdftoppm", pdftoppm_available()) {
        return;
    }'''
assert s.count(old_vis) == 1, "one visual-similarity gate expected"
s = s.replace(old_vis, new_vis, 1)

# sanity: exactly one ensure_tool definition, one require_pdf_tools
assert s.count("fn ensure_tool(name: &str, available: bool) -> bool {") == 1
assert s.count("fn require_pdf_tools() -> bool {") == 1
assert s.count("FailingGpu") >= 3, "GPU tests must still be present"

p.write_text(s)
print("OK: gates applied; ensure_tool x1, require_pdf_tools x1, FailingGpu present")
