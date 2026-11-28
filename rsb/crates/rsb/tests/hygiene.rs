//! Source checks that need no window: what the tests print must not claim a session it did not run in.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target" || n == "golden") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A timing line says "rdp" or "console" from the session it ran in, never a fixed word.
#[test]
fn no_printed_line_names_a_fixed_session() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(files.len() > 50, "found only {} source files", files.len());
    let (open, close) = (["(", "rdp"].concat(), ["rdp", ")"].concat());
    let mut bad = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            if line.contains(&open) || line.contains(&close) {
                bad.push(format!("{}:{}", file.file_name().unwrap().to_string_lossy(), n + 1));
            }
        }
    }
    assert!(bad.is_empty(), "a fixed session label is printed at {bad:?}");
}
