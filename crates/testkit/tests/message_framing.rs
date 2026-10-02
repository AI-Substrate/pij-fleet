use std::fs;
use std::path::{Path, PathBuf};

const RUST_HEAD_LITERAL: &str = "\"[pij-rs from ";

fn rust_sources_below(dir: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
    {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_sources_below(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

#[test]
fn all_pij_rs_renderers_delegate_to_one_formatter() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let owner = workspace.join("crates/core/src/framing.rs");
    let crates = workspace.join("crates");
    let mut sources = Vec::new();
    rust_sources_below(&crates, &mut sources);

    let mut duplicate_owners = Vec::new();
    for path in sources {
        if path == owner || !path.components().any(|part| part.as_os_str() == "src") {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        if production.contains(RUST_HEAD_LITERAL) {
            duplicate_owners.push(
                path.strip_prefix(&workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }

    assert!(
        duplicate_owners.is_empty(),
        "pij-rs frame head must be owned only by crates/core/src/framing.rs; use \
         pij_core::framing::frame_message instead of rendering at {duplicate_owners:?}"
    );
}
