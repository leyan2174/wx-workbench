use std::{fs, path::Path};

#[allow(dead_code)]
pub fn authenticated_transport(root: &Path, out: &Path) {
    generate_transport(root, out, false);
}

#[allow(dead_code)]
pub fn authenticated_task_transport(root: &Path, out: &Path) {
    generate_transport(root, out, true);
}

fn generate_transport(root: &Path, out: &Path, tasks: bool) {
    let framing = root.join("../../../src/service/transport/framing.rs");
    println!("cargo:rerun-if-changed={}", framing.display());
    fs::create_dir_all(out.join("transport")).unwrap();
    fs::copy(framing, out.join("transport/framing.rs")).unwrap();
    let mut calls = vec![
        "WorkerDatabaseKeys",
        "WorkerImageMaterial",
        "Mcp",
        "Info",
        "Shutdown",
    ];
    let mut types = vec!["Call", "Envelope", "Reply", "ServiceError"];
    if tasks {
        calls.extend(["Configure", "Submit", "List", "Get", "Cancel", "Events", "TaskArtifacts", "ReadTaskArtifact"]);
        types.extend(["Kind", "Format", "Options", "Submission"]);
    }
    let source = root.join("../../../src/service/protocol.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
    ast.attrs.clear();
    ast.items.retain(|item| match item {
        syn::Item::Const(value) => ["VERSION", "MAX_REQUEST_BYTES", "MAX_RESPONSE_BYTES"].iter().any(|name| value.ident == name),
        syn::Item::Enum(value) => types.iter().any(|name| value.ident == name),
        syn::Item::Struct(value) => types.iter().any(|name| value.ident == name),
        syn::Item::Impl(value) => matches!(value.self_ty.as_ref(), syn::Type::Path(path)
            if types.iter().any(|name| path.path.is_ident(name))),
        syn::Item::Fn(value) => tasks && ["valid_task_id", "parse_task_kind", "is_false"].iter().any(|name| value.sig.ident == name),
        _ => false,
    });
    for item in &mut ast.items {
        match item {
            syn::Item::Enum(value) if value.ident == "Call" => {
                value.variants = value.variants.clone().into_iter()
                    .filter(|variant| calls.iter().any(|name| variant.ident == name)).collect();
                assert_eq!(value.variants.len(), calls.len());
            },
            syn::Item::Impl(value) if matches!(value.self_ty.as_ref(), syn::Type::Path(path) if path.path.is_ident("Call")) => {
                for item in &mut value.items {
                    if let syn::ImplItem::Fn(method) = item {
                        for statement in &mut method.block.stmts {
                            if let syn::Stmt::Expr(syn::Expr::Match(value), _) = statement {
                                value.arms.retain(|arm| match &arm.pat {
                                    syn::Pat::Struct(value) => value.path.segments.last().is_some_and(|segment|
                                        calls.iter().any(|name| segment.ident == name)),
                                    _ => true,
                                });
                            }
                        }
                    }
                }
            },
            _ => {},
        }
    }
    ast.items.insert(0, syn::parse_quote!(use serde::{Deserialize, Serialize};));
    ast.items.insert(1, syn::parse_quote!(use serde_json::Value;));
    if tasks {
        ast.items.insert(2, syn::parse_quote!(use super::settings::SettingsInput;));
    }
    fs::write(out.join("service_protocol.rs"), prettyplease::unparse(&ast)).unwrap();
    generate_database_worker_keys(root, out);
    let mut modules = vec!["client", "transport", "query_client"];
    if tasks {
        modules.extend(["config_pin", "plan", "settings", "task_artifacts"]);
        let source = root.join("../../../src/cli/tasks.rs");
        println!("cargo:rerun-if-changed={}", source.display());
        let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
        ast.attrs.clear();
        ast.items.retain(|item| matches!(item, syn::Item::Fn(value)
            if ["validate_export_options", "supports_artifacts"].iter().any(|name| value.sig.ident == name)));
        assert_eq!(ast.items.len(), 2, "Production task validation helpers changed");
        ast.items.insert(0, syn::parse_quote!(use anyhow::{ensure, Result};));
        ast.items.insert(1, syn::parse_quote!(use serde_json::Value;));
        ast.items.insert(2, syn::parse_quote!(use crate::service::protocol::{Kind, Submission};));
        fs::write(out.join("cli_task_validation.rs"), prettyplease::unparse(&ast)).unwrap();
    }
    for name in modules {
        let source = root.join(format!("../../../src/service/{name}.rs"));
        println!("cargo:rerun-if-changed={}", source.display());
        let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
        ast.attrs.retain(|attribute| !attribute.path().is_ident("doc"));
        ast.items.retain(|item| !matches!(item, syn::Item::Mod(value) if value.ident == "tests"));
        if name == "query_client" {
            for item in &mut ast.items {
                if let syn::Item::Fn(value) = item {
                    if value.sig.ident == "start_daemon" {
                        value.attrs.push(syn::parse_quote!(#[allow(unused_variables)]));
                        *value.block = syn::parse_quote!({
                            bail!("fixture never starts a daemon")
                        });
                    }
                }
            }
        }
        fs::write(out.join(format!("service_{name}.rs")), prettyplease::unparse(&ast)).unwrap();
    }
}

fn generate_database_worker_keys(root: &Path, out: &Path) {
    let source = root.join("../../../src/service/worker_keys.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
    let types = [
        "Secret",
        "DatabaseReadRequest",
        "DatabaseKeys",
        "DatabaseSnapshot",
        "ImageReadRequest",
        "ImageMaterial",
        "ImageSnapshot",
        "DatabaseReplyReader",
    ];
    let constants = [
        "DATABASE_REPLY_MAGIC",
        "MAX_DATABASE_REPLY_BYTES",
        "MAX_DATABASE_KEYS",
        "MAX_DATABASE_NAME_BYTES",
        "IMAGE_REPLY_MAGIC",
        "MAX_IMAGE_REPLY_BYTES",
    ];
    let functions = [
        "error_tag",
        "tagged_error",
        "tagged_image_error",
        "put_bytes",
        "encode_database_reply",
        "decode_database_reply",
        "encode_image_reply",
        "decode_image_reply",
    ];
    ast.attrs.clear();
    ast.items.retain(|item| match item {
        syn::Item::Struct(value) => types.iter().any(|name| value.ident == name),
        syn::Item::Impl(value) => matches!(value.self_ty.as_ref(), syn::Type::Path(path)
            if path.path.segments.last().is_some_and(|segment|
                types.iter().any(|name| segment.ident == name))),
        syn::Item::Const(value) => constants.iter().any(|name| value.ident == name),
        syn::Item::Fn(value) => functions.iter().any(|name| value.sig.ident == name),
        _ => false,
    });
    ast.items.insert(0, syn::parse_quote!(use std::{collections::HashMap, fmt};));
    ast.items.insert(1, syn::parse_quote!(use anyhow::{anyhow, ensure, Result};));
    ast.items.insert(2, syn::parse_quote!(use serde::{Deserialize, Serialize};));
    ast.items.insert(3, syn::parse_quote!(use zeroize::Zeroize;));
    ast.items.push(syn::parse_quote!(#[cfg(test)] fn observe_drop(_: bool) {}));
    fs::write(
        out.join("service_worker_keys.rs"),
        prettyplease::unparse(&ast),
    )
    .unwrap();
}
