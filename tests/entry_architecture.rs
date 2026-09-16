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

struct ProductionDependency {
    forbidden: &'static [&'static str],
    found: bool,
}

impl ProductionDependency {
    fn record(&mut self, segments: &[String]) {
        self.found |= self.forbidden.iter().any(|pattern| {
            let expected: Vec<_> = pattern.split("::").collect();
            segments
                .windows(expected.len())
                .any(|part| part == expected)
        });
    }

    fn import(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.import(&path.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.import(tree, prefix);
                }
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.record(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.record(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => {
                self.record(prefix);
                // A wildcard on an engine parent can import any forbidden child.
                self.found |= self.forbidden.iter().any(|pattern| {
                    let expected: Vec<_> = pattern.split("::").collect();
                    (1..expected.len()).any(|n| {
                        prefix.ends_with(
                            &expected[..n]
                                .iter()
                                .map(|s| s.to_string())
                                .collect::<Vec<_>>(),
                        )
                    })
                });
            }
        }
    }
}

impl<'ast> Visit<'ast> for ProductionDependency {
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
        self.record(
            &path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
        );
        syn::visit::visit_path(self, path);
    }

    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        self.import(tree, &mut Vec::new());
    }
}

fn production_depends_on_cli(source: &str) -> bool {
    production_depends_on(source, &["cli"])
}

#[test]
fn private_file_infrastructure_does_not_depend_on_execution_or_toolkit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let source = fs::read_to_string(root.join("private_file.rs")).unwrap();
    assert!(!production_depends_on(
        &source,
        &[
            "crate::toolkit",
            "crate::daemon",
            "crate::cli",
            "crate::business",
        ]
    ));
    assert!(!root.join("toolkit/private_file.rs").exists());
}

#[test]
fn output_tree_infrastructure_does_not_depend_on_business_or_entry_layers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("infrastructure")) {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !production_depends_on(
                &source,
                &[
                    "crate::adapters",
                    "crate::business",
                    "crate::cli",
                    "crate::daemon",
                    "crate::mcp",
                    "crate::service",
                    "crate::toolkit",
                ],
            ),
            "Infrastructure depends on an upper layer: {}",
            file.display()
        );
    }
    assert!(!root.join("toolkit/directory_publish").exists());
}

#[test]
fn application_monitor_does_not_depend_on_execution_or_wechat_implementation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("application/monitor")) {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !production_depends_on(
                &source,
                &[
                    "crate::adapters",
                    "crate::cli",
                    "crate::daemon",
                    "crate::toolkit",
                ],
            ),
            "Application monitor depends on an entry, execution, or WeChat layer: {}",
            file.display()
        );
    }
    assert!(!root.join("toolkit/monitor.rs").exists());
    assert!(!root.join("toolkit/monitor").exists());
}

fn production_depends_on(source: &str, forbidden: &'static [&'static str]) -> bool {
    let syntax = syn::parse_file(source).expect("valid Rust source");
    let mut dependencies = ProductionDependency {
        forbidden,
        found: false,
    };
    if !test_only(&syntax.attrs) {
        dependencies.visit_file(&syntax);
    }
    dependencies.found
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
        .chain(sources(&root.join("web")))
        .chain(sources(&root.join("mcp")))
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

const FRONTEND_ENGINES: &[&str] = &[
    "application::transcription",
    "infrastructure::transcription",
    "application::moments",
    "infrastructure::audio",
    "application::voice_batch_export",
    "application::chat_directory",
    "application::chat_delta_export",
    "application::chat_export_plan",
    "application::chat_archive_index",
    "application::chat_archive_merge",
    "application::chat_plan_selection",
    "application::emoticons",
    "application::image_publication::parse_aes",
    "attachment::local_files",
    "attachment::decoder",
    "scanner::scan_keys",
    "DbCache",
    "infrastructure::publication::atomic_output",
    "infrastructure::configuration::ConfigDocument",
];

#[test]
fn frontend_adapters_do_not_import_business_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("cli"))
        .into_iter()
        .chain(sources(&root.join("web")))
        .chain(sources(&root.join("mcp")))
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
        assert!(
            !production_depends_on(&source, FRONTEND_ENGINES),
            "Frontend imports a business engine: {}",
            file.display()
        );
    }
}

#[test]
fn executable_business_dispatch_is_only_in_internal_daemon_workers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in sources(&root.join("cli")) {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !production_depends_on(&source, &["operations::execute"]),
            "CLI executes daemon operation locally: {}",
            file.display()
        );
        assert!(
            !production_depends_on(&source, &["task_worker::run"]),
            "CLI owns task execution: {}",
            file.display()
        );
    }
    let client = fs::read_to_string(root.join("service/operation_client.rs")).unwrap();
    assert!(production_depends_on(&client, &["Call::OperationStart"]));
    assert!(!production_depends_on(&client, &["operations::execute"]));
    let worker = fs::read_to_string(root.join("daemon/operation_worker.rs")).unwrap();
    assert!(production_depends_on(
        &worker,
        &["daemon::operations::execute"]
    ));
}

#[test]
fn dispatch_guard_tracks_rust_dependencies_not_local_variable_names_or_strings() {
    for source in [
        "fn run() { crate::daemon::operations::execute(operation); }",
        "fn run() { crate :: daemon :: operations :: execute(request.operation); }",
        "use crate::daemon::operations::execute as dispatch; fn run() { dispatch(value); }",
    ] {
        assert!(production_depends_on(
            source,
            &["daemon::operations::execute"]
        ));
    }
    for source in [
        "const EXAMPLE: &str = \"crate::daemon::operations::execute(operation)\";",
        "// crate::daemon::operations::execute(operation)\nfn run() {}",
        "fn run() { another::execute(request.operation); }",
        "#[cfg(test)] fn fixture() { crate::daemon::operations::execute(operation); }",
    ] {
        assert!(!production_depends_on(
            source,
            &["daemon::operations::execute"]
        ));
    }
}

#[test]
fn frontend_guard_understands_rust_paths_and_test_boundaries() {
    for source in [
        "use crate::application::{transcription as engine};",
        "use crate::infrastructure::{audio as engine};",
        "use crate::application::*;",
        "fn run() { crate :: scanner :: scan_keys(); }",
        "type Cache = crate::daemon::cache::DbCache;",
    ] {
        assert!(
            production_depends_on(source, FRONTEND_ENGINES),
            "missed {source}"
        );
    }
    for source in [
        "// application::transcription
struct Request;",
        "const EXAMPLE: &str = \"application::moments::export\";",
        "#[cfg(test)] mod tests { use crate::infrastructure::audio; }",
        "use crate::service::operation_requests::Voices;",
    ] {
        assert!(
            !production_depends_on(source, FRONTEND_ENGINES),
            "false positive: {source}"
        );
    }
}
