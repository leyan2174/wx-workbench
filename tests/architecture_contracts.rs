use std::{fs, path::Path};
use syn::visit::Visit;

#[derive(Default)]
struct Dependencies {
    paths: Vec<Vec<String>>,
}

impl<'ast> Visit<'ast> for Dependencies {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("derive") {
            if let Ok(paths) = attribute.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            ) {
                for path in paths {
                    self.paths
                        .push(path.segments.iter().map(|s| s.ident.to_string()).collect());
                }
            }
        }
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.paths
            .push(path.segments.iter().map(|s| s.ident.to_string()).collect());
        syn::visit::visit_path(self, path);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        fn walk(tree: &syn::UseTree, prefix: Vec<String>, paths: &mut Vec<Vec<String>>) {
            match tree {
                syn::UseTree::Path(path) => {
                    let mut prefix = prefix;
                    prefix.push(path.ident.to_string());
                    walk(&path.tree, prefix, paths);
                }
                syn::UseTree::Group(group) => {
                    for tree in &group.items {
                        walk(tree, prefix.clone(), paths);
                    }
                }
                syn::UseTree::Name(name) => {
                    let mut prefix = prefix;
                    prefix.push(name.ident.to_string());
                    paths.push(prefix);
                }
                syn::UseTree::Rename(rename) => {
                    let mut prefix = prefix;
                    prefix.push(rename.ident.to_string());
                    paths.push(prefix);
                }
                syn::UseTree::Glob(_) => paths.push(prefix),
            }
        }
        walk(&item.tree, Vec::new(), &mut self.paths);
    }
}

fn violations(source: &str, business: bool) -> Vec<String> {
    let syntax = syn::parse_file(source).expect("valid Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    dependencies
        .paths
        .into_iter()
        .filter(|path| {
            path.iter()
                .any(|part| matches!(part.as_str(), "daemon" | "clap" | "cli"))
                || (business
                    && (path.iter().any(|part| {
                        matches!(part.as_str(), "rusqlite" | "adapters" | "wechat_data")
                    }) || path.windows(2).any(|pair| {
                        pair[0] == "serde_json" && matches!(pair[1].as_str(), "Value" | "json")
                    })))
        })
        .map(|path| path.join("::"))
        .collect()
}

fn inspect(path: &Path, business: bool) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            inspect(&entry.unwrap().path(), business);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        let problems = violations(&fs::read_to_string(path).unwrap(), business);
        assert!(problems.is_empty(), "{}: {problems:?}", path.display());
    }
}

#[test]
fn public_contracts_do_not_depend_on_execution_or_cli_parsing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service");
    for name in ["operation_requests", "operations.rs", "mcp.rs"] {
        inspect(&root.join(name), false);
    }
}

#[test]
fn business_models_do_not_depend_on_storage_adapters_or_json_projection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/business");
    assert!(root.is_dir());
    inspect(&root, true);
}

#[test]
fn syntax_check_ignores_comments_and_catches_renamed_imports() {
    assert!(violations(
        "// use crate::daemon; serde_json::Value\nstruct Contact;",
        true
    )
    .is_empty());
    for source in [
        "use serde_json::{Value as Json};",
        "use crate::adapters::contacts as source;",
        "fn query(_: rusqlite::Connection) {}",
        "#[derive(clap::Args)] struct Args {}",
    ] {
        assert!(!violations(source, true).is_empty(), "missed {source}");
    }
}
