use std::path::Path;

use pro_gui::context::*;

#[test]
fn scan_and_pack_this_repo() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tree = scan_workspace(root);
    assert!(
        tree.files.iter().any(|f| f.rel_path == "src/main.rs"),
        "src/main.rs should be discovered"
    );
    assert!(
        !tree.files.iter().any(|f| f.rel_path.starts_with("target/")),
        "gitignored target/ leaked into the scan"
    );

    let selected = vec!["src/main.rs".to_owned(), "Cargo.toml".to_owned()];
    let packed = pack_context(root, &selected, true);
    assert_eq!(packed.included, 2);
    assert!(packed.text.contains("### src/main.rs"));
    assert!(packed.text.contains("```rust"));
    assert!(packed.missing.is_empty());
}

#[test]
fn secret_scan_finds_keys() {
    let content =
        "let key = \"sk-abcdefghijklmnopqrstuvwx123456\";\nok line\nAKIAABCDEFGHIJKLMNOP\n";
    let hits = scan_secrets("config.rs", content);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].line, 1);
    assert_eq!(hits[1].kind, "AWS access key id");
}

#[test]
fn filter_terms_combine() {
    let paths = [
        "app/api/views.py",
        "app/api/tests/test_views.py",
        "app/web/views.py",
        "app/migrations/0001_initial.py",
        "frontend/src/api/client.ts",
        "README.md",
    ];
    let matching = |query: &str| {
        let filter = PathFilter::parse(query);
        paths
            .iter()
            .copied()
            .filter(|path| filter.matches(path))
            .collect::<Vec<_>>()
    };

    assert_eq!(matching("VIEWS !tests").len(), 2);
    assert_eq!(matching("*.py !migrations !test_*").len(), 2);
    assert_eq!(
        matching("api/*.py"),
        ["app/api/views.py", "app/api/tests/test_views.py"]
    );
    assert_eq!(matching("/frontend/*"), ["frontend/src/api/client.ts"]);
    assert!(
        matching("/api/*").is_empty(),
        "a leading / anchors at the root"
    );
    assert_eq!(matching("readme.??"), ["README.md"]);
    assert_eq!(matching("").len(), paths.len());
    assert!(PathFilter::parse("  ").is_empty());
}

#[test]
fn extensions_ignore_dotfiles_and_case() {
    assert_eq!(extension_of("src/App.TSX"), "tsx");
    assert_eq!(extension_of("archive.tar.gz"), "gz");
    assert_eq!(extension_of(".env"), "");
    assert_eq!(extension_of("Makefile"), "");
}

#[test]
fn token_counting_is_sane() {
    let count = count_tokens("fn main() { println!(\"hello world\"); }");
    assert!(count > 5 && count < 40, "unexpected token count {count}");
}
