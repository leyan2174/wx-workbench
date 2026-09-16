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
fn moments_application_does_not_interpret_wechat_storage() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/application/moments/export.rs");
    let source = fs::read_to_string(&path).unwrap();
    let syntax = syn::parse_file(&source).expect("valid Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    let sqlite = dependencies.paths.into_iter().find(|path| {
        path.iter()
            .any(|segment| segment.eq_ignore_ascii_case("rusqlite"))
    });
    assert!(sqlite.is_none(), "{}: {sqlite:?}", path.display());
    for storage_detail in ["SELECT ", "PRAGMA ", "SnsTimeLine", "SnsMessage_tmp3"] {
        assert!(
            !source.contains(storage_detail),
            "{} contains WeChat storage detail {storage_detail}",
            path.display()
        );
    }
}

#[test]
fn database_decryption_delegates_sqlite_validation_to_infrastructure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let application_path = root.join("application/database_decryption.rs");
    let application = fs::read_to_string(&application_path).unwrap();
    let syntax = syn::parse_file(&application).expect("valid Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    let sqlite = dependencies.paths.into_iter().find(|path| {
        path.iter()
            .any(|segment| segment.eq_ignore_ascii_case("rusqlite"))
    });
    assert!(
        sqlite.is_none(),
        "{}: {sqlite:?}",
        application_path.display()
    );
    for storage_detail in ["PRAGMA ", "Connection::", "OpenFlags::"] {
        assert!(
            !application.contains(storage_detail),
            "{} contains SQLite infrastructure detail {storage_detail}",
            application_path.display()
        );
    }

    let infrastructure_path = root.join("infrastructure/sqlite_validation.rs");
    let infrastructure = fs::read_to_string(&infrastructure_path).unwrap();
    assert!(
        infrastructure.contains("PRAGMA quick_check"),
        "{} must retain the integrity check delegated by the application",
        infrastructure_path.display()
    );
}

#[test]
fn chat_directory_receives_image_material_without_opening_the_key_store() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/application/chat_directory");
    for path in [root.with_extension("rs"), root.join("media.rs")] {
        let source = fs::read_to_string(&path).unwrap();
        let syntax = syn::parse_file(&source).expect("valid Rust source");
        let mut dependencies = Dependencies::default();
        dependencies.visit_file(&syntax);
        let key_store = dependencies
            .paths
            .into_iter()
            .find(|path| path.iter().any(|segment| segment == "key_store"));
        assert!(key_store.is_none(), "{}: {key_store:?}", path.display());
        assert!(
            !source.contains("Store::for_runtime") && !source.contains("Store::for_config"),
            "{} opens the account key store",
            path.display()
        );
    }
}

#[test]
fn image_publication_receives_material_without_opening_the_key_store() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/application/image_publication.rs");
    let source = fs::read_to_string(&path).unwrap();
    let module_name = source
        .find("mod publication_tests")
        .expect("publication tests module");
    let test_module = source[..module_name]
        .rfind("#[cfg(test)]")
        .expect("publication tests attribute");
    let production = &source[..test_module];
    let syntax = syn::parse_file(production).expect("valid production Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    let key_store = dependencies
        .paths
        .into_iter()
        .find(|path| path.iter().any(|segment| segment == "key_store"));
    assert!(key_store.is_none(), "{}: {key_store:?}", path.display());
    assert!(
        !production.contains("Store::for_runtime") && !production.contains("Store::for_config"),
        "{} opens the account key store",
        path.display()
    );
    assert!(
        production.contains("StoredImageKeys"),
        "{} must consume explicit host-provided material",
        path.display()
    );
}

#[test]
fn sns_archive_receives_image_material_without_opening_the_key_store() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/daemon/operations/sns_archive.rs");
    let source = fs::read_to_string(&path).unwrap();
    let syntax = syn::parse_file(&source).expect("valid Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    let key_store = dependencies
        .paths
        .into_iter()
        .find(|path| path.iter().any(|segment| segment == "key_store"));
    assert!(key_store.is_none(), "{}: {key_store:?}", path.display());
    assert!(
        !source.contains("Store::for_runtime") && !source.contains("Store::for_config"),
        "{} opens the account key store",
        path.display()
    );
}

#[test]
fn sns_timeline_reads_image_material_through_worker_broker() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/daemon/operations/sns_timeline.rs");
    let source = fs::read_to_string(&path).unwrap();
    let production = source.rsplit_once("#[cfg(test)]").unwrap().0;
    let syntax = syn::parse_file(production).expect("valid production Rust source");
    let mut dependencies = Dependencies::default();
    dependencies.visit_file(&syntax);
    let key_store = dependencies
        .paths
        .into_iter()
        .find(|path| path.iter().any(|segment| segment == "key_store"));
    assert!(key_store.is_none(), "{}: {key_store:?}", path.display());
    assert!(
        !production.contains("Store::for_runtime") && !production.contains("Store::for_config"),
        "{} opens the account key store",
        path.display()
    );
    assert!(
        production.contains("service::worker_keys::image_material")
            && production.contains("service::worker_keys::verify_image_revision"),
        "{} must use the process-bound worker key broker",
        path.display()
    );
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
