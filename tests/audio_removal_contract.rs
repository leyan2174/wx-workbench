//! Removed derivations must not silently return through another entry point.
use std::{fs, path::Path};

fn has_rust_source(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    fs::read_dir(path).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            has_rust_source(&path)
        } else {
            path.extension().is_some_and(|ext| ext == "rs")
        }
    })
}

#[test]
fn production_has_no_audio_derivation_sources_or_codec_dependency() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for directory in [
        "src/application/transcription",
        "src/infrastructure/audio",
        "src/infrastructure/transcription",
    ] {
        assert!(
            !has_rust_source(&root.join(directory)),
            "removed production source remains: {directory}"
        );
    }
    assert!(!root.join("src/application/voice_batch_export.rs").exists());
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("silk-codec"),
        "removed codec dependency remains"
    );
}

#[test]
fn typed_operations_and_steps_do_not_reintroduce_removed_variants() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, name, forbidden) in [
        (
            "src/service/operations.rs",
            "Operation",
            &[
                "TranscribeAudio",
                "TranscribeChat",
                "TranscribeBatch",
                "TranscribeDatabase",
                "VoiceExport",
                "ExportAudio",
                "ConvertAudio",
            ][..],
        ),
        (
            "src/service/protocol.rs",
            "Kind",
            &["Transcribe", "VoiceExport", "VoiceMp3"][..],
        ),
        (
            "src/service/plan.rs",
            "Step",
            &["Transcribe", "TranscribeChats", "VoiceExport", "VoiceBatch"][..],
        ),
    ] {
        let source = syn::parse_file(&fs::read_to_string(root.join(file)).unwrap()).unwrap();
        let item = source
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) if item.ident == name => Some(item),
                _ => None,
            })
            .expect("production enum must remain inspectable");
        for variant in &item.variants {
            assert!(
                !forbidden.contains(&variant.ident.to_string().as_str()),
                "{file}: removed variant {}",
                variant.ident
            );
        }
    }
}
