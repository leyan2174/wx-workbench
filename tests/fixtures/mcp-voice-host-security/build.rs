use std::{env, fs, path::PathBuf};
#[path = "../mcp-auth/build_support.rs"]
mod support;
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    support::authenticated_task_transport(&root, &out);
    for (name, relative, target, tag) in [
        ("mcp_service", "daemon/mcp_service.rs", "open", "account-open"),
        ("mcp_voice", "daemon/mcp_service/voice.rs", "host_path", "host-path"),
        ("asr", "daemon/operations/asr.rs", "build", "backend-build"),
    ] {
        let source = root.join("../../../src").join(relative);
        println!("cargo:rerun-if-changed={}", source.display());
        let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
        ast.attrs.retain(|a| !a.path().is_ident("doc"));
        ast.items.retain(|item| !matches!(item, syn::Item::Mod(value) if value.ident == "tests" || value.ident == "configured_local_tests"));
        // These imports belong solely to the test modules stripped above.
        let test_import: Option<syn::ItemUse> = match name {
            "mcp_voice" => Some(syn::parse_quote!(use crate::service::operation_requests::asr::BackendKind;)),
            "mcp_service" => Some(syn::parse_quote!(use crate::service::mcp::unpack;)),
            _ => None,
        };
        if let Some(expected) = test_import {
            let expected = expected.tree;
            let expected = quote::quote!(#expected).to_string();
            ast.items.retain(|item| match item {
                syn::Item::Use(import) => {
                    let tree = &import.tree;
                    quote::quote!(#tree).to_string() != expected
                }
                _ => true,
            });
        }
        if name == "mcp_service" {
            for item in &mut ast.items {
                if matches!(item, syn::Item::Mod(value) if value.ident == "voice") {
                    *item = syn::parse_quote!(pub use crate::mcp_voice as voice;);
                }
            }
        }
        let stmt: syn::Stmt = syn::parse_quote! { eprintln!("AUDIT_EVENT:{}", #tag); };
        let mut count = 0;
        for item in &mut ast.items {
            match item {
                syn::Item::Fn(f) if f.sig.ident == target => {
                    f.block.stmts.insert(0, stmt.clone());
                    count += 1;
                }
                syn::Item::Impl(i) => {
                    if name == "mcp_service" && !matches!(i.self_ty.as_ref(), syn::Type::Path(path) if path.path.is_ident("PinnedAccount")) {
                        continue;
                    }
                    for member in &mut i.items {
                        if let syn::ImplItem::Fn(f) = member {
                            if f.sig.ident == target {
                                f.block.stmts.insert(0, stmt.clone());
                                count += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(count, 1, "unique audit probe {name}:{target}");
        fs::write(out.join(format!("{name}.rs")), prettyplease::unparse(&ast)).unwrap();
    }
}
