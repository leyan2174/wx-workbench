//! Guard the business ownership boundary without requiring a live account.
use std::{
    fs,
    path::{Path, PathBuf},
};
use syn::visit::Visit;

fn test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(attribute.parse_args::<syn::Meta>(), Ok(syn::Meta::Path(path)) if path.is_ident("test"))
    })
}

#[derive(Default)]
struct ProductionCliDependency(bool);

impl<'ast> Visit<'ast> for ProductionCliDependency {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attributes = match item {
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::ExternCrate(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::ForeignMod(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::TraitAlias(item) => &item.attrs,
            syn::Item::Type(item) => &item.attrs,
            syn::Item::Union(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            _ => {
                syn::visit::visit_item(self, item);
                return;
            }
        };
        if !test_only(attributes) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.0 |= path.segments.iter().any(|segment| segment.ident == "cli");
        syn::visit::visit_path(self, path);
    }

    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        self.0 |= match tree {
            syn::UseTree::Path(path) => path.ident == "cli",
            syn::UseTree::Name(name) => name.ident == "cli",
            syn::UseTree::Rename(rename) => rename.ident == "cli",
            _ => false,
        };
        syn::visit::visit_use_tree(self, tree);
    }
}

fn production_depends_on_cli(source: &str) -> bool {
    let syntax = syn::parse_file(source).expect("valid Rust source");
    let mut dependencies = ProductionCliDependency::default();
    if !test_only(&syntax.attrs) {
        dependencies.visit_file(&syntax);
    }
    dependencies.0
}

fn sources(path: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            result.push(path);
        }
    }
    result
}

#[test]
fn daemon_and_shared_services_do_not_depend_on_cli_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("daemon"))
        .into_iter()
        .chain(sources(&root.join("service")))
        .chain(sources(&root.join("toolkit/web")))
    {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !production_depends_on_cli(&source),
            "Execution, service or Web module depends on CLI: {}",
            file.display()
        );
    }
}

#[test]
fn production_dependency_guard_only_excludes_explicit_test_items() {
    for source in [
        "use crate::cli::Args;",
        "use crate::{cli as frontend};",
        "fn execute(_: crate::cli::Args) {}",
        "mod tests { use crate::cli::Args; }",
        "#[cfg(any(test, windows))] mod bridge { use crate::cli::Args; }",
    ] {
        assert!(production_depends_on_cli(source), "missed {source}");
    }
    for source in [
        "// use crate::cli::Args;\nstruct Request;",
        "const EXAMPLE: &str = \"crate::cli::Args\";",
        "#[cfg(test)] mod tests { use crate::cli::Args; }",
        "#[cfg(test)] fn parser_contract(_: crate::cli::Args) {}",
        "#![cfg(test)]\nuse crate::cli::Args;",
    ] {
        assert!(
            !production_depends_on_cli(source),
            "false positive: {source}"
        );
    }
}

#[test]
fn frontend_adapters_do_not_import_business_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "toolkit::asr",
        "toolkit::sns::",
        "toolkit::audio",
        "toolkit::chat_directory",
        "toolkit::chat_delta",
        "toolkit::chat_merge",
        "toolkit::emoticons",
        "toolkit::parse_image_aes",
        "attachment::local_files",
        "attachment::decoder",
        "scanner::scan_keys",
        "DbCache",
        "toolkit::atomic_output",
        "toolkit::setup::ConfigDocument",
    ];
    for file in sources(&root.join("cli"))
        .into_iter()
        .chain(sources(&root.join("toolkit/web")))
    {
        if file
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .ends_with("_tests")
        {
            continue;
        }
        let source = fs::read_to_string(&file).unwrap();
        for import in forbidden {
            assert!(
                !source.contains(import),
                "Frontend imports {import}: {}",
                file.display()
            );
        }
    }
}

#[test]
fn executable_business_dispatch_is_only_in_internal_daemon_workers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("cli")) {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !source.contains("operations::execute("),
            "CLI executes daemon operation locally: {}",
            file.display()
        );
        assert!(
            !source.contains("task_worker::run("),
            "CLI owns task execution: {}",
            file.display()
        );
    }
    let client = fs::read_to_string(root.join("service/operation_client.rs")).unwrap();
    assert!(client.contains("Call::OperationStart"));
    assert!(!client.contains("operations::execute("));
    let worker = fs::read_to_string(root.join("daemon/operation_worker.rs")).unwrap();
    assert!(worker.contains("crate::daemon::operations::execute(operation)"));
}
