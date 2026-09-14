//! Guard the business ownership boundary without requiring a live account.
use std::{
    fs,
    path::{Path, PathBuf},
};

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
    {
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            !source.contains("crate::cli::"),
            "Service depends on a CLI module: {}",
            file.display()
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
