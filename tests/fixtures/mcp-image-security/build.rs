use quote::ToTokens;
fn main() {
    let path = "../../../src/cli/transport.rs";
    println!("cargo:rerun-if-changed={path}");
    let source = std::fs::read_to_string(path).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let function = parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "read_response" => Some(function),
            _ => None,
        })
        .expect("actual production read_response must exist");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        output.join("ipc_reader.rs"),
        function.to_token_stream().to_string(),
    )
    .unwrap();
    let wav_path = "../../../src/toolkit/asr/mod.rs";
    println!("cargo:rerun-if-changed={wav_path}");
    let wav = syn::parse_file(&std::fs::read_to_string(wav_path).unwrap()).unwrap();
    let wav_items: Vec<_> = wav
        .items
        .iter()
        .filter(|item| match item {
            syn::Item::Const(value) => value.ident == "MAX_AUDIO_BYTES",
            syn::Item::Struct(value) => value.ident == "WavInfo",
            syn::Item::Fn(value) => value.sig.ident == "validate_wav",
            _ => false,
        })
        .collect();
    assert_eq!(wav_items.len(), 3);
    std::fs::write(
        output.join("wav_validator.rs"),
        quote::quote!(#(#wav_items)*).to_string(),
    )
    .unwrap();

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
    let positions: Vec<_> = function
        .block
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| {
            if let syn::Stmt::Expr(syn::Expr::Try(value), _) = statement {
                if let syn::Expr::MethodCall(call) = value.expr.as_ref() {
                    if call.method == "sync_all" {
                        return Some(index);
                    }
                }
            }
            None
        })
        .collect();
    assert_eq!(
        positions.len(),
        1,
        "one precise after-sync instrumentation point required"
    );
    function.block.stmts.insert(
        positions[0] + 1,
        syn::parse_quote!(crate::publish_probe::after_image_sync();),
    );
    std::fs::write(
        output.join("instrumented_image.rs"),
        image.to_token_stream().to_string(),
    )
    .unwrap();
}
