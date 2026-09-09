//! Every file in `examples/` must parse with zero diagnostics, and its AST is
//! locked down by a snapshot.

mod common;

use std::path::{Path, PathBuf};

use typhoon_diag::{Diagnostics, SourceMap, render_all};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn example_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("examples directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ty"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no examples found");
    files
}

#[test]
fn every_example_parses_without_diagnostics() {
    for path in example_files() {
        let text = std::fs::read_to_string(&path).expect("readable example");
        let mut sources = SourceMap::new();
        let file = sources.add(path.display().to_string(), text.clone());
        let mut diags = Diagnostics::new();
        let module = typhoon_parser::parse_module(file, &text, &mut diags);
        assert!(
            diags.is_empty(),
            "{} produced diagnostics:\n{}",
            path.display(),
            render_all(diags.iter(), &sources, false)
        );
        assert!(
            !module.items.is_empty(),
            "{} parsed to an empty module",
            path.display()
        );
        assert!(
            module.items.iter().any(|item| item.name().name == "main"),
            "{} has no `fn main()`",
            path.display()
        );
    }
}

#[test]
fn example_asts() {
    for path in example_files() {
        let text = std::fs::read_to_string(&path).expect("readable example");
        let mut diags = Diagnostics::new();
        let module = typhoon_parser::parse_module(common::FILE, &text, &mut diags);
        assert!(diags.is_empty(), "{} produced diagnostics", path.display());
        let name = path
            .file_stem()
            .expect("file stem")
            .to_string_lossy()
            .to_string();
        insta::assert_snapshot!(name, typhoon_ast::dump(&module));
    }
}
