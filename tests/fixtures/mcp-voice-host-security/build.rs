use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (name, target, tag) in [
        ("mcp", "open_config_read_lock", "account-open"),
        ("mcp_voice", "host_path", "host-path"),
        ("asr", "build", "backend-build"),
    ] {
        let source = root.join(format!("../../../src/cli/{name}.rs"));
        println!("cargo:rerun-if-changed={}", source.display());
        let mut ast = syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap();
        ast.attrs.retain(|a| !a.path().is_ident("doc"));
        let stmt: syn::Stmt = syn::parse_quote! { eprintln!("AUDIT_EVENT:{}", #tag); };
        let mut count = 0;
        for item in &mut ast.items {
            match item {
                syn::Item::Fn(f) if f.sig.ident == target => {
                    f.block.stmts.insert(0, stmt.clone());
                    count += 1;
                }
                syn::Item::Impl(i) => {
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
