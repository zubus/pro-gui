use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("src/ is readable") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn unit_tests_live_in_their_own_tests_rs_files() {
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );

    let offenders: Vec<String> = files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name != "tests.rs"))
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("source file is readable");
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    let line = line.trim();
                    line.starts_with("#[test]") || line.starts_with("mod tests {")
                })
                .map(|(index, _)| format!("{}:{}", path.display(), index + 1))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "tests belong in a sibling tests.rs (declared with `#[cfg(test)] mod tests;`) or in tests/: {offenders:#?}"
    );
}
