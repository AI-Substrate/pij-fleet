//! The extension source trees cover what the extensions load (F113).
//!
//! The PA digest flags a seat whose extension source differs from what the
//! daemon was built with, by comparing git tree ids: each extension reports its
//! directory's tree (and the OMP extension the one file it imports from outside
//! it), and `just rs-install` / `just bounce-rs` stamp the same. A tree only
//! covers what is under it, so this walks each extension's relative imports
//! from its entry point and fails when the closure reaches a file outside the
//! directory that the identity does not list.
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Every quoted string that starts `./` or `../`, wherever it stands. Not
/// keyword leads: a comment or a line break between `from` or `import(` and
/// the quote hid an import from them (#175 review). Reading every relative
/// literal, imports or not, can only add files to the closure, so the check
/// can fail spuriously but never pass blind.
fn relative_specifiers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for quote in ['"', '\'', '`'] {
        for lead in ["./", "../"] {
            let open = format!("{quote}{lead}");
            let mut rest = text;
            while let Some(at) = rest.find(&open) {
                let after = &rest[at + 1..];
                if let Some(end) = after.find(quote) {
                    let spec = &after[..end];
                    if !spec.contains('\n') {
                        found.push(spec.to_string());
                    }
                }
                rest = after;
            }
        }
    }
    found
}

#[test]
fn the_scanner_sees_imports_however_they_are_written() {
    let json = "../../../crates/core/tests/fixtures/control-commands.json";
    for source in [
        format!("import fixture from \"{json}\" with {{ type: \"json\" }};"),
        format!("import fixture from /* closure probe */ \"{json}\";"),
        format!("const fixture = await import(\n  \"{json}\", {{ with: {{ type: \"json\" }} }});"),
        format!("export {{ a }} from\n'{json}';"),
        format!("const x = require( `{json}` );"),
    ] {
        assert_eq!(relative_specifiers(&source), [json], "{source}");
    }
    assert!(relative_specifiers("import x from \"pkg\"; const y = \"a./b\";").is_empty());
    assert_eq!(
        relative_specifiers("import(`./${name}.js`)"),
        ["./${name}.js"]
    );
}

/// `path` with `.` and `..` folded away, without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The file a relative import names, as TypeScript and Node resolve it here:
/// `./x.js` is `./x.ts` in TypeScript sources.
fn resolve(from: &Path, spec: &str) -> Option<PathBuf> {
    let base = normalize(&from.parent().expect("a file's directory").join(spec));
    let text = base.to_string_lossy().to_string();
    let mut candidates = Vec::new();
    if let Some(stem) = text.strip_suffix(".js") {
        candidates.push(PathBuf::from(format!("{stem}.ts")));
    }
    candidates.push(base.clone());
    for extension in ["ts", "mjs", "js"] {
        candidates.push(PathBuf::from(format!("{text}.{extension}")));
    }
    candidates.push(base.join("index.ts"));
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Every repository-relative file the entry point loads, transitively.
fn closure(root: &Path, entry: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.join(entry)];
    while let Some(file) = stack.pop() {
        let relative = file
            .strip_prefix(root)
            .expect("inside the repository")
            .to_string_lossy()
            .to_string();
        if !seen.insert(relative) {
            continue;
        }
        if file
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("read a source file");
        for spec in relative_specifiers(&text) {
            assert!(
                !spec.contains("${"),
                "{} loads a computed relative path `{spec}` the closure cannot follow; \
                 name the file, or list its root in the extension's identity",
                file.display()
            );
            if let Some(target) = resolve(&file, &spec) {
                stack.push(target);
            }
        }
    }
    seen
}

fn outside(files: &BTreeSet<String>, directory: &str) -> Vec<String> {
    files
        .iter()
        .filter(|file| !file.starts_with(&format!("{directory}/")))
        .cloned()
        .collect()
}

#[test]
fn each_extension_tree_covers_its_import_closure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root");

    let copilot = closure(&root, ".copilot/extensions/pij/extension.mjs");
    assert!(
        copilot.len() >= 2,
        "the scan found {copilot:?}; fix the scanner"
    );
    assert_eq!(
        outside(&copilot, ".copilot/extensions/pij"),
        Vec::<String>::new(),
        "the Copilot extension's identity is its directory's tree alone; an import \
         outside it needs a root in its identity (store.mjs) and the justfile stamp"
    );

    let omp = closure(&root, ".omp/extensions/pij/index.ts");
    assert!(
        omp.len() >= 20,
        "the scan found only {} files; fix the scanner",
        omp.len()
    );
    let listed = "crates/core/tests/fixtures/control-commands.json";
    assert_eq!(
        outside(&omp, ".omp/extensions/pij"),
        [listed],
        "the OMP extension's identity covers its directory and OUTSIDE_ROOTS in \
         adapters/extension-build.ts; add any new outside import there and to the \
         justfile's PIJ_EXT_TREE_OMP stamp"
    );

    // Both ends name the same outside root.
    let identity =
        std::fs::read_to_string(root.join(".omp/extensions/pij/adapters/extension-build.ts"))
            .expect("read the OMP identity");
    assert!(
        identity.contains(&format!("\"../../../{listed}\"")),
        "OUTSIDE_ROOTS lists {listed}"
    );
    let justfile = std::fs::read_to_string(root.join("justfile")).expect("read the justfile");
    let stamp = format!(
        "git rev-parse HEAD HEAD:.copilot/extensions/pij HEAD:.omp/extensions/pij HEAD:{listed}"
    );
    assert_eq!(
        justfile.matches(&stamp).count(),
        2,
        "just rs-install and just bounce-rs stamp the same trees"
    );
}
