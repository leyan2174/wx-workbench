//! Pinned image input/configuration and protected publication; WeChat rules live in the adapter.
use super::{export_context::ExportContext as ImageScope, files::*, Failure, Report};
#[cfg(test)]
use crate::attachment::decoder::{dispatch, V2KeyMaterial, V2_MAGIC};
use crate::{
    adapters::wechat::media::image_batch::{self, DecodeMode, Decoded, KeyMaterial, Layout},
    attachment::local_files::{HostOutputGuard, Pin},
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn parse_aes(value: &str) -> Result<[u8; 16]> {
    ensure!(
        value.is_ascii() && value.len() >= 16,
        "Image AES key requires at least 16 ASCII bytes"
    );
    Ok(value.as_bytes()[..16].try_into().unwrap())
}

pub(super) fn parse_xor(value: &str) -> Result<u8> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Ok(u8::from_str_radix(hex, 16).context("Invalid XOR key")?)
    } else {
        Ok(value.parse().context("Invalid XOR key")?)
    }
}
fn image_keys(
    cfg: &crate::config::Config,
    aes: Option<String>,
    xor: Option<String>,
) -> Result<(Option<[u8; 16]>, u8)> {
    let aes = aes.as_deref().map(parse_aes).transpose()?;
    let xor = xor.as_deref().map(parse_xor).transpose()?;
    let stored = if aes.is_some() && (xor.is_some() || cfg.key_store.is_none()) {
        (None, 0x88)
    } else {
        crate::key_store::Store::for_config(cfg)?
            .load()?
            .image_material()
    };
    let stored = zeroize::Zeroizing::new(stored);
    Ok((aes.or(stored.0), xor.unwrap_or(stored.1)))
}

fn image_keys_scoped(
    scope: &ImageScope,
    aes: Option<String>,
    xor: Option<String>,
) -> Result<(Option<[u8; 16]>, u8)> {
    scope.verify()?;
    let keys = match scope.account() {
        Some(runtime) => image_keys(&runtime.config, aes, xor)?,
        None => (
            aes.as_deref().map(parse_aes).transpose()?,
            xor.as_deref().map(parse_xor).transpose()?.unwrap_or(0x88),
        ),
    };
    scope.verify()?;
    Ok(keys)
}

fn raw_config(scope: &ImageScope) -> Result<Value> {
    scope.verify()?;
    let value = serde_json::from_slice(&fs::read(scope.config_path())?)?;
    scope.verify()?;
    Ok(value)
}

fn publish(
    scope: &ImageScope,
    input: &ImageInput,
    root: &Path,
    target: &Path,
    bytes: &[u8],
) -> Result<()> {
    scope.verify()?;
    input.verify()?;
    let mut protected = scope.protected(root)?;
    protected.push(input.path.clone());
    ExportTarget::capture_paths(target, &protected)?.write_bytes_checked(bytes, || {
        #[cfg(test)]
        publication_probe();
        scope.verify()?;
        input.verify()
    })
}

struct ImageInput {
    path: PathBuf,
    parent: HostOutputGuard,
    pin: Pin,
}
impl ImageInput {
    fn read(path: &Path) -> Result<(Self, Vec<u8>)> {
        let path = std::path::absolute(path)?;
        let parent = HostOutputGuard::new(path.parent().context("Image input has no parent")?)?;
        let pin = Pin::open(&path, false)?;
        let input = Self { path, parent, pin };
        input.verify()?;
        // The pinned handle denies writes/deletes; the path identity is checked around the read.
        let bytes = fs::read(&input.path)?;
        input.verify()?;
        Ok((input, bytes))
    }

    fn verify(&self) -> Result<()> {
        self.parent.verify()?;
        self.pin.verify()
    }
}

#[cfg(test)]
fn explicit_path_image_keys(
    config_path: &Path,
    aes: Option<String>,
    xor: Option<String>,
) -> Result<(Option<[u8; 16]>, u8)> {
    let scope = ImageScope::at(
        config_path,
        config_path
            .parent()
            .context("Missing config parent")?
            .join("runtime"),
    )?;
    image_keys_scoped(&scope, aes, xor)
}

pub fn decode_image(input: String, output: Option<String>) -> Result<()> {
    decode_image_scoped(&ImageScope::current()?, input, output)
}

fn decode_image_scoped(scope: &ImageScope, input: String, output: Option<String>) -> Result<()> {
    scope.runtime()?;
    let input = std::path::absolute(input)?;
    if let Some(output) = &output {
        validate_export_paths(Path::new(output), &scope.protected(&input)?)?;
    }
    let material = zeroize::Zeroizing::new(image_keys_scoped(scope, None, None)?);
    let (source, bytes) = ImageInput::read(&input)?;
    let Decoded::Image(decoded) = image_batch::decode(
        &bytes,
        KeyMaterial {
            aes_key: material.0.as_ref(),
            xor_key: material.1,
        },
        DecodeMode::Single,
    )?
    else {
        anyhow::bail!("Single image decoding requires key material")
    };
    let output = match output {
        Some(path) => PathBuf::from(path),
        None => image_batch::single_output(&input, decoded.format)?,
    };
    publish(scope, &source, &input, &output, &decoded.data)?;
    println!(
        "{}",
        serde_json::json!({"output":output,"format":decoded.format,"bytes":decoded.data.len(),"engine":"rust"})
    );
    Ok(())
}

pub fn decode_images(
    input: Option<String>,
    output: Option<String>,
    aes: Option<String>,
    xor: Option<String>,
    force: bool,
) -> Result<()> {
    decode_images_scoped(&ImageScope::current()?, input, output, aes, xor, force)
}

/// Task hosts pass their already selected runtime, rather than discovering configuration again.
pub(crate) fn decode_images_for(
    runtime: &RuntimeContext,
    input: Option<String>,
    output: Option<String>,
    aes: Option<String>,
    xor: Option<String>,
    force: bool,
) -> Result<()> {
    decode_images_scoped(
        &ImageScope::for_runtime(runtime)?,
        input,
        output,
        aes,
        xor,
        force,
    )
}

fn decode_images_scoped(
    scope: &ImageScope,
    input: Option<String>,
    output: Option<String>,
    aes: Option<String>,
    xor: Option<String>,
    force: bool,
) -> Result<()> {
    let input = match input {
        Some(path) => PathBuf::from(path),
        None => image_batch::default_input(&scope.runtime()?.config.db_dir)?,
    };
    let output = match output {
        Some(path) => PathBuf::from(path),
        None => {
            scope.runtime()?;
            let raw = raw_config(scope)?;
            scope
                .config_path()
                .parent()
                .context("Missing configuration parent")?
                .join(
                    raw.get("decoded_image_dir")
                        .and_then(Value::as_str)
                        .unwrap_or("decoded_images"),
                )
        }
    };
    let material = zeroize::Zeroizing::new(image_keys_scoped(scope, aes, xor)?);
    batch_scoped(
        scope,
        &input,
        &output,
        material.0.as_ref(),
        material.1,
        force,
        Layout::Album,
    )?
    .finish()
}

pub fn batch_images(input: String, output: Option<String>) -> Result<()> {
    batch_images_scoped(&ImageScope::current()?, input, output)
}

fn batch_images_scoped(scope: &ImageScope, input: String, output: Option<String>) -> Result<()> {
    scope.runtime()?;
    let material = zeroize::Zeroizing::new(image_keys_scoped(scope, None, None)?);
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(format!("{}_decoded", input.trim_end_matches(['\\', '/'])))
    });
    batch_scoped(
        scope,
        Path::new(&input),
        &output,
        material.0.as_ref(),
        material.1,
        false,
        Layout::Mirror,
    )?
    .finish()
}

fn existing_output(target: &Path, layout: Layout) -> Result<bool> {
    let parent = target.parent().context("Missing output directory")?;
    if !parent.exists() {
        return Ok(false);
    }
    let prefix = format!(
        "{}.",
        target
            .file_name()
            .context("Missing output filename")?
            .to_string_lossy()
    )
    .to_lowercase();
    for entry in fs::read_dir(parent)? {
        if layout.existing_name(&prefix, &entry?.file_name().to_string_lossy()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn batch_scoped(
    scope: &ImageScope,
    input: &Path,
    output: &Path,
    aes: Option<&[u8; 16]>,
    xor: u8,
    force: bool,
    layout: Layout,
) -> Result<Report> {
    enum Outcome {
        Written(&'static str),
        Existing,
        NoKey,
    }
    scope.verify()?;
    let input = std::path::absolute(input)?;
    let output = std::path::absolute(output)?;
    validate_export_paths(&output, &scope.protected(&input)?)?;
    let input_guard = HostOutputGuard::new(&input)?;
    let files = collect(&input, image_batch::INPUT_EXTENSION, false)?;
    let mut report = Report::default();
    for file in files {
        let rel = file.strip_prefix(&input)?;
        let Some(target) = layout.target(rel, &output)? else {
            continue;
        };
        report.total += 1;
        let result = (|| -> Result<Outcome> {
            scope.verify()?;
            input_guard.verify()?;
            if !force && existing_output(&target, layout)? {
                return Ok(Outcome::Existing);
            }
            let (source, bytes) = ImageInput::read(&file)?;
            let decoded = match image_batch::decode(
                &bytes,
                KeyMaterial {
                    aes_key: aes,
                    xor_key: xor,
                },
                DecodeMode::Batch,
            )? {
                Decoded::MissingKey => return Ok(Outcome::NoKey),
                Decoded::Image(image) => image,
            };
            let target = image_batch::with_format(&target, decoded.format);
            publish(scope, &source, &input, &target, &decoded.data)?;
            Ok(Outcome::Written(decoded.format))
        })();
        match result {
            Ok(Outcome::Written(format)) => {
                report.written += 1;
                *report.formats.entry(format.into()).or_default() += 1;
            }
            Ok(Outcome::Existing) => report.skipped += 1,
            Ok(Outcome::NoKey) => report.skipped_no_key += 1,
            Err(error) => report.failures.push(Failure {
                path: rel.into(),
                error: error.to_string(),
            }),
        }
        if report.total % 200 == 0 {
            eprintln!(
                "Images: {} processed, {} written, {} skipped, {} skipped_no_key, {} failed",
                report.total,
                report.written,
                report.skipped,
                report.skipped_no_key,
                report.failures.len()
            );
        }
    }
    scope.verify()?;
    input_guard.verify()?;
    Ok(report)
}

#[cfg(test)]
pub(super) fn batch(
    input: &Path,
    output: &Path,
    aes: Option<&[u8; 16]>,
    xor: u8,
    force: bool,
    album_layout: bool,
) -> Result<Report> {
    let parent = input.parent().context("Synthetic input needs a parent")?;
    let scope = ImageScope::at(
        &parent.join("absent-image-test-config.json"),
        parent.join("runtime"),
    )?;
    batch_scoped(
        &scope,
        input,
        output,
        aes,
        xor,
        force,
        if album_layout {
            Layout::Album
        } else {
            Layout::Mirror
        },
    )
}

#[cfg(test)]
std::thread_local! {
    static BEFORE_IMAGE_PUBLISH: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}
#[cfg(test)]
fn publication_probe() {
    BEFORE_IMAGE_PUBLISH.with(|hook| {
        if let Some(callback) = hook.borrow_mut().take() {
            callback();
        }
    });
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::key_store::{Store, Update, Verification};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic publication image";

    fn put(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, PNG.iter().map(|byte| byte ^ 0x37).collect::<Vec<_>>()).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, RuntimeContext) {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("all_keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            key_store: Some(root.path().join("keys.dpapi")),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir).unwrap();
        fs::create_dir_all(&config.decrypted_dir).unwrap();
        fs::write(&config.keys_file, b"synthetic protected keys").unwrap();
        let path = root.path().join("config.json");
        let mut raw = serde_json::to_value(&config).unwrap();
        raw["decoded_image_dir"] = "custom-decoded".into();
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let runtime =
            RuntimeContext::from_config(path, config, root.path().join("runtime")).unwrap();
        Store::for_config(&runtime.config)
            .unwrap()
            .update(Some(0), &[Update::ImageXor(0x88, Verification::Verified)])
            .unwrap();
        (root, runtime)
    }

    struct ProbeReset;
    impl Drop for ProbeReset {
        fn drop(&mut self) {
            BEFORE_IMAGE_PUBLISH.with(|hook| *hook.borrow_mut() = None);
        }
    }
    fn probe(callback: impl FnOnce() + 'static) -> ProbeReset {
        BEFORE_IMAGE_PUBLISH.with(|hook| {
            assert!(hook.borrow().is_none());
            *hook.borrow_mut() = Some(Box::new(callback));
        });
        ProbeReset
    }

    #[test]
    fn single_decode_cannot_replace_config_keys_store_account_cache_runtime_or_input() {
        let (root, runtime) = fixture();
        let input = root.path().join("source.dat");
        put(&input);
        let scope = ImageScope::for_runtime(&runtime).unwrap();
        let protected = [
            runtime.config_path.clone(),
            runtime.config.keys_file.clone(),
            runtime.config.key_store.clone().unwrap(),
            runtime.config.db_dir.join("image.png"),
            runtime.config.decrypted_dir.join("image.png"),
            runtime.cache_dir().join("image.png"),
            runtime.directory.join("image.png"),
            input.clone(),
        ];
        let before: Vec<_> = protected.iter().map(|path| fs::read(path).ok()).collect();
        for (target, before) in protected.iter().zip(&before) {
            assert!(
                decode_image_scoped(
                    &scope,
                    input.to_string_lossy().into_owned(),
                    Some(target.to_string_lossy().into_owned())
                )
                .is_err(),
                "{}",
                target.display()
            );
            assert_eq!(&fs::read(target).ok(), before);
        }
    }

    #[test]
    fn album_mirror_and_task_entries_reject_protected_output_roots() {
        let (root, runtime) = fixture();
        let input = root.path().join("input");
        put(&input.join("peer/2026-09/Img/photo.dat"));
        let scope = ImageScope::for_runtime(&runtime).unwrap();
        for output in [
            runtime.config_path.clone(),
            runtime.config.keys_file.clone(),
            runtime.config.key_store.clone().unwrap(),
            runtime.config.db_dir.clone(),
            runtime.config.decrypted_dir.clone(),
            runtime.directory.clone(),
        ] {
            let source = input.to_string_lossy().into_owned();
            let target = output.to_string_lossy().into_owned();
            assert!(decode_images_scoped(
                &scope,
                Some(source.clone()),
                Some(target.clone()),
                None,
                None,
                true
            )
            .is_err());
            assert!(batch_images_scoped(&scope, source.clone(), Some(target.clone())).is_err());
            assert!(
                decode_images_for(&runtime, Some(source), Some(target), None, None, true).is_err()
            );
        }
        assert_eq!(
            fs::read(&runtime.config.keys_file).unwrap(),
            b"synthetic protected keys"
        );
    }

    #[test]
    fn normal_single_mirror_and_task_defaults_preserve_paths_and_exact_bytes() {
        let (root, runtime) = fixture();
        let scope = ImageScope::for_runtime(&runtime).unwrap();
        let single = root.path().join("single_t.dat");
        put(&single);
        decode_image_scoped(&scope, single.to_string_lossy().into_owned(), None).unwrap();
        assert_eq!(fs::read(root.path().join("single.png")).unwrap(), PNG);
        let mirror = root.path().join("mirror");
        put(&mirror.join("nested/photo_h.dat"));
        batch_images_scoped(&scope, mirror.to_string_lossy().into_owned(), None).unwrap();
        assert_eq!(
            fs::read(root.path().join("mirror_decoded/nested/photo.png")).unwrap(),
            PNG
        );
        let album = image_batch::default_input(&runtime.config.db_dir).unwrap();
        put(&album.join("peer/2026-09/Img/photo_t.dat"));
        decode_images_for(&runtime, None, None, None, None, false).unwrap();
        assert_eq!(
            fs::read(root.path().join("custom-decoded/peer/2026-09/photo.png")).unwrap(),
            PNG
        );
    }

    #[test]
    fn explicit_album_paths_and_aes_remain_usable_without_configuration() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("absent.json");
        let scope = ImageScope::at(&config, root.path().join("runtime")).unwrap();
        let input = root.path().join("input");
        let output = root.path().join("output");
        put(&input.join("peer/2026-09/Img/photo.dat"));
        decode_images_scoped(
            &scope,
            Some(input.to_string_lossy().into_owned()),
            Some(output.to_string_lossy().into_owned()),
            Some("synthetic-key-16".into()),
            Some("0x88".into()),
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("peer/2026-09/photo.png")).unwrap(),
            PNG
        );
        assert!(!config.exists());
        assert!(decode_images_scoped(
            &scope,
            None,
            Some(output.to_string_lossy().into_owned()),
            None,
            None,
            false
        )
        .is_err());
    }

    #[test]
    fn configuration_appearing_at_final_publication_preserves_previous_output() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("absent.json");
        let scope = ImageScope::at(&config, root.path().join("runtime")).unwrap();
        let input = root.path().join("input");
        let source = input.join("peer/2026-09/Img/photo.dat");
        put(&source);
        let output = root.path().join("output");
        let previous = output.join("peer/2026-09/photo.png");
        fs::create_dir_all(previous.parent().unwrap()).unwrap();
        fs::write(&previous, b"previous output").unwrap();
        let _probe = probe(move || fs::write(&config, b"configuration appeared").unwrap());
        assert!(batch_scoped(&scope, &input, &output, None, 0x88, true, Layout::Album).is_err());
        assert_eq!(fs::read(&previous).unwrap(), b"previous output");
        assert_eq!(fs::read_dir(previous.parent().unwrap()).unwrap().count(), 1);
        assert!(source.exists());
    }

    #[test]
    fn configured_publication_pins_source_and_config_against_write_delete_and_rename() {
        let (root, runtime) = fixture();
        let scope = ImageScope::for_runtime(&runtime).unwrap();
        let input = root.path().join("source.dat");
        let output = root.path().join("published.png");
        put(&input);
        let config = runtime.config_path.clone();
        let source = input.clone();
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let _probe = probe(move || {
            assert!(fs::write(&source, b"changed source").is_err());
            assert!(fs::rename(&source, source.with_extension("moved")).is_err());
            assert!(fs::write(&config, b"changed configuration").is_err());
            assert!(fs::remove_file(&config).is_err());
            observed.store(true, Ordering::SeqCst);
        });
        decode_image_scoped(
            &scope,
            input.to_string_lossy().into_owned(),
            Some(output.to_string_lossy().into_owned()),
        )
        .unwrap();
        assert!(called.load(Ordering::SeqCst));
        assert_eq!(fs::read(output).unwrap(), PNG);
        scope.verify().unwrap();
    }

    #[test]
    fn task_runtime_cannot_be_rebound_to_changed_configuration() {
        let (root, runtime) = fixture();
        let mut changed = serde_json::to_value(&runtime.config).unwrap();
        changed["keys_file"] = root
            .path()
            .join("different-keys.json")
            .to_string_lossy()
            .into_owned()
            .into();
        fs::write(&runtime.config_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let output = root.path().join("output");
        assert!(decode_images_for(
            &runtime,
            None,
            Some(output.to_string_lossy().into_owned()),
            Some("synthetic-key-16".into()),
            Some("0x88".into()),
            false
        )
        .is_err());
        assert!(!output.exists());
    }
}

#[cfg(test)]
mod encrypted_image_tests {
    use super::*;
    use crate::key_store::{Error, Store, Update, Verification};

    #[test]
    fn explicit_image_paths_allow_absent_config_but_never_fallback_from_a_store() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.json");
        let (aes, xor) = explicit_path_image_keys(&path, None, None)?;
        assert_eq!((aes, xor), (None, 0x88));
        let plain = b"\x89PNG\r\n\x1a\nsynthetic image";
        let bytes: Vec<_> = plain.iter().map(|byte| byte ^ 0x37).collect();
        assert_eq!(
            dispatch(
                &bytes,
                V2KeyMaterial {
                    aes_key: aes.as_ref(),
                    xor_key: xor
                }
            )?
            .data,
            plain
        );
        assert!(dispatch(
            &V2_MAGIC,
            V2KeyMaterial {
                aes_key: None,
                xor_key: xor
            }
        )
        .is_err());
        let mut config = serde_json::json!({"db_dir":"db_storage", "keys_file":"all_keys.json"});
        fs::create_dir(root.path().join("db_storage"))?;
        fs::write(&path, serde_json::to_vec(&config)?)?;
        assert!(matches!(
            explicit_path_image_keys(&path, None, None)
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::LegacyMigrationRequired)
        ));
        config["key_store"] = serde_json::json!("keys.dpapi");
        fs::write(&path, serde_json::to_vec(&config)?)?;
        assert!(matches!(
            explicit_path_image_keys(&path, None, None)
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::Missing)
        ));
        fs::write(
            root.path().join("keys.dpapi"),
            b"synthetic corrupted ciphertext",
        )?;
        assert!(explicit_path_image_keys(&path, None, None).is_err());
        fs::write(&path, b"invalid synthetic configuration")?;
        assert!(explicit_path_image_keys(&path, None, None).is_err());
        Ok(())
    }

    #[test]
    fn encrypted_image_reader_requires_store_and_preserves_explicit_overrides() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut config = crate::config::Config {
            key_store: Some(root.path().join("keys.dpapi")),
            db_dir: root.path().join("db_storage"),
            keys_file: root.path().join("all_keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: String::new(),
        };
        fs::create_dir(&config.db_dir)?;
        let store = Store::for_config(&config)?;
        assert!(matches!(
            image_keys(&config, None, None)
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::Missing)
        ));
        let aes = *b"syntheticAESkey1";
        let xor_only = store.update(Some(0), &[Update::ImageXor(0xa2, Verification::Verified)])?;
        assert_eq!(xor_only.image_key(), None);
        assert_eq!(image_keys(&config, None, None)?, (None, 0xa2));
        assert_eq!(
            image_keys(&config, Some("explicitAESkey12".into()), None)?,
            (Some(*b"explicitAESkey12"), 0xa2)
        );
        store.update(
            Some(xor_only.revision()),
            &[Update::Image(&aes, 0xa2, Verification::Verified)],
        )?;
        assert_eq!(image_keys(&config, None, None)?, (Some(aes), 0xa2));
        assert_eq!(
            image_keys(&config, Some("explicitAESkey12".into()), None)?,
            (Some(*b"explicitAESkey12"), 0xa2)
        );
        fs::write(store.path(), b"corrupted")?;
        assert!(image_keys(&config, None, None).is_err());
        assert!(image_keys(&config, Some("explicitAESkey12".into()), None).is_err());
        assert_eq!(
            image_keys(
                &config,
                Some("explicitAESkey12".into()),
                Some("0x51".into())
            )?,
            (Some(*b"explicitAESkey12"), 0x51)
        );
        config.key_store = None;
        assert!(matches!(
            image_keys(&config, None, None)
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::LegacyMigrationRequired)
        ));
        assert_eq!(
            image_keys(&config, Some("explicitAESkey12".into()), None)?,
            (Some(*b"explicitAESkey12"), 0x88)
        );
        Ok(())
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use crate::attachment::decoder::V1_MAGIC;
    use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};

    const KEY: &[u8; 16] = b"synthetic-key-16";
    const JPEG: &[u8] = b"\xff\xd8\xffsynthetic";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic";

    fn aes_image(magic: [u8; 6], key: &[u8; 16]) -> Vec<u8> {
        let padding = 16 - JPEG.len() % 16;
        let mut padded = JPEG.to_vec();
        padded.resize(JPEG.len() + padding, padding as u8);
        let aes = aes::Aes128::new(key.into());
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&(JPEG.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0);
        for chunk in padded.chunks_exact(16) {
            let mut block = GenericArray::clone_from_slice(chunk);
            aes.encrypt_block(&mut block);
            bytes.extend_from_slice(&block);
        }
        bytes
    }

    fn mixed_input(root: &Path, album: bool) -> PathBuf {
        let input = root.join("input");
        let leaf = if album {
            input.join("chat/2026-09/Img")
        } else {
            input.clone()
        };
        fs::create_dir_all(&leaf).unwrap();
        fs::write(
            leaf.join("v1.dat"),
            aes_image(V1_MAGIC, b"cfcd208495d565ef"),
        )
        .unwrap();
        fs::write(leaf.join("v2.dat"), aes_image(V2_MAGIC, KEY)).unwrap();
        fs::write(
            leaf.join("xor.dat"),
            PNG.iter().map(|byte| byte ^ 0x37).collect::<Vec<_>>(),
        )
        .unwrap();
        input
    }

    fn assert_totals(report: &Report) {
        assert_eq!(
            report.total,
            report.written + report.skipped + report.skipped_no_key + report.failures.len()
        );
        assert_eq!(report.formats.values().sum::<usize>(), report.written);
    }

    #[test]
    fn missing_v2_key_preserves_v1_xor_and_recovers_with_key_in_both_layouts() {
        for album in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let input = mixed_input(root.path(), album);
            let output = root.path().join("output");
            let targets = if album {
                output.join("chat/2026-09")
            } else {
                output.clone()
            };
            let report = batch(&input, &output, None, 0x88, false, album).unwrap();
            assert_totals(&report);
            assert_eq!(
                (
                    report.total,
                    report.written,
                    report.skipped,
                    report.skipped_no_key
                ),
                (3, 2, 0, 1)
            );
            assert!(report.failures.is_empty());
            assert_eq!(report.formats.get("jpg"), Some(&1));
            assert_eq!(report.formats.get("png"), Some(&1));
            assert_eq!(fs::read(targets.join("v1.jpg")).unwrap(), JPEG);
            assert_eq!(fs::read(targets.join("xor.png")).unwrap(), PNG);
            assert!(!targets.join("v2.jpg").exists());
            assert!(report.finish().is_ok());
            let json = serde_json::to_value(&report).unwrap();
            for field in [
                "total",
                "written",
                "skipped",
                "planned",
                "failures",
                "skipped_no_key",
                "formats",
            ] {
                assert!(json.get(field).is_some());
            }
            let resumed = batch(&input, &output, Some(KEY), 0x88, false, album).unwrap();
            assert_totals(&resumed);
            assert_eq!(
                (resumed.written, resumed.skipped, resumed.skipped_no_key),
                (1, 2, 0)
            );
            assert!(resumed.failures.is_empty());
            assert_eq!(resumed.formats.len(), 1);
            assert_eq!(resumed.formats.get("jpg"), Some(&1));
            assert_eq!(fs::read(targets.join("v2.jpg")).unwrap(), JPEG);
            let existing = batch(&input, &output, None, 0x88, false, album).unwrap();
            assert_totals(&existing);
            assert_eq!((existing.skipped, existing.skipped_no_key), (3, 0));
            assert!(existing.formats.is_empty());
        }
    }

    #[test]
    fn wrong_aes_key_is_failure_not_missing_key_and_keeps_previous_output() {
        let root = tempfile::tempdir().unwrap();
        let input = mixed_input(root.path(), false);
        let output = root.path().join("output");
        let first = batch(&input, &output, Some(KEY), 0x88, false, false).unwrap();
        assert_eq!(first.written, 3);
        let report = batch(
            &input,
            &output,
            Some(b"wrong-key-16byte"),
            0x88,
            true,
            false,
        )
        .unwrap();
        assert_totals(&report);
        assert_eq!(
            (report.written, report.skipped_no_key, report.failures.len()),
            (2, 0, 1)
        );
        assert_eq!(report.failures[0].path, Path::new("v2.dat"));
        assert_eq!(fs::read(output.join("v2.jpg")).unwrap(), JPEG);
        assert!(report.finish().is_err());
    }

    #[test]
    fn corrupt_v2_ciphertext_with_key_fails_while_other_formats_continue() {
        let root = tempfile::tempdir().unwrap();
        let input = mixed_input(root.path(), false);
        let mut corrupt = aes_image(V2_MAGIC, KEY);
        corrupt.truncate(20);
        fs::write(input.join("v2.dat"), corrupt).unwrap();
        let output = root.path().join("output");
        let report = batch(&input, &output, Some(KEY), 0x88, false, false).unwrap();
        assert_totals(&report);
        assert_eq!(
            (report.written, report.skipped_no_key, report.failures.len()),
            (2, 0, 1)
        );
        assert!(!output.join("v2.jpg").exists());
        assert_eq!(report.formats.get("jpg"), Some(&1));
        assert_eq!(report.formats.get("png"), Some(&1));
        assert!(report.finish().is_err());
    }

    #[test]
    fn missing_key_only_does_not_create_output_or_count_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input");
        fs::create_dir(&input).unwrap();
        fs::write(input.join("v2.dat"), aes_image(V2_MAGIC, KEY)).unwrap();
        let output = root.path().join("output");
        let report = batch(&input, &output, None, 0x88, false, false).unwrap();
        assert_totals(&report);
        assert_eq!(report.skipped_no_key, 1);
        assert!(report.failures.is_empty());
        assert!(report.formats.is_empty());
        assert!(!output.exists());
        assert!(report.finish().is_ok());
    }
}
