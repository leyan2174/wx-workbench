use quote::ToTokens;
fn main() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let image_path = "../../../src/attachment/native_image.rs";
    println!("cargo:rerun-if-changed={image_path}");
    let mut image = syn::parse_file(&std::fs::read_to_string(image_path).unwrap()).unwrap();
    image
        .attrs
        .retain(|attribute| !attribute.path().is_ident("doc"));
    let function = image
        .items
        .iter_mut()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "export_image_impl" => Some(function),
            _ => None,
        })
        .expect("production image implementation must exist");
    let mut points = 0;
    for statement in &mut function.block.stmts {
        let syn::Stmt::Expr(syn::Expr::Try(value), _) = statement else { continue };
        let mut expression = value.expr.as_mut();
        while let syn::Expr::MethodCall(call) = expression {
            if call.method == "write_bytes_checked" {
                let syn::Expr::Closure(callback) = call.args.iter_mut().nth(1).unwrap() else {
                    panic!("publication callback must remain explicit");
                };
                let syn::Expr::Block(body) = callback.body.as_mut() else {
                    panic!("publication callback must remain a block");
                };
                body.block.stmts.insert(0, syn::parse_quote!(crate::publish_probe::after_image_sync();));
                points += 1;
                break;
            }
            expression = call.receiver.as_mut();
        }
    }
    assert_eq!(
        points,
        1,
        "one precise after-sync instrumentation point required"
    );
    std::fs::write(
        output.join("instrumented_image.rs"),
        image.to_token_stream().to_string(),
    )
    .unwrap();
}
