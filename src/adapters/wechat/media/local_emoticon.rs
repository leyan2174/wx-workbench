//! Local-only catalog consumption. No URLs, keys, or catalog rows leave this module.
use super::local_read::{bounded_read, hash32, Scan};
use crate::{
    adapters::wechat::emoticons::CatalogSource,
    attachment::{decoder, local_files::Pin},
    business::media::{Error, Failure, Stage},
};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    MessageDigest,
    CatalogAndMessageDigest,
}
impl Binding {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MessageDigest => "message_md5",
            Self::CatalogAndMessageDigest => "materialized_catalog_message_md5",
        }
    }
}
pub(crate) struct Resolved {
    pub bytes: Vec<u8>,
    pub format: &'static str,
    pub binding: Binding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_catalog_binding_and_explicit_legacy_do_not_publish_or_fetch() {
        let root = tempfile::tempdir().unwrap();
        let images = root.path().join("images");
        let stage = root.path().join("stage");
        std::fs::create_dir(&images).unwrap();
        std::fs::create_dir(&stage).unwrap();
        let bytes = b"\xff\xd8\xffsynthetic local image\xff\xd9";
        let digest = format!("{:x}", md5::compute(bytes));
        std::fs::write(images.join(format!("{digest}.jpg")), bytes).unwrap();
        let legacy = resolve(&images, None, &digest, &stage, 4096, 4096).unwrap();
        assert_eq!(legacy.binding, Binding::MessageDigest);
        assert_eq!(legacy.bytes, bytes);
        let path = root.path().join("catalog.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!(
            "../../../../tests/fixtures/emoticons-catalog/schema.sql"
        ))
        .unwrap();
        conn.execute("INSERT INTO kNonStoreEmoticonTable VALUES(?1,'synthetic-secret','https://example.invalid/private','','p')", [&digest]).unwrap();
        drop(conn);
        let known = resolve(&images, Some(&path), &digest, &stage, 4096, 4096).unwrap();
        assert_eq!(known.binding, Binding::CatalogAndMessageDigest);
        assert_eq!(known.bytes, bytes);
        std::fs::write(&path, b"invalid catalog").unwrap();
        let error = resolve(&images, Some(&path), &digest, &stage, 4096, 4096)
            .err()
            .unwrap();
        assert_eq!(error.failure, Failure::IncompleteSources);
        assert!(!error.to_string().contains("synthetic-secret"));
        assert_eq!(std::fs::read_dir(&stage).unwrap().count(), 0);
    }
}

pub(crate) fn resolve(
    root: &Path,
    catalog_path: Option<&Path>,
    digest: &str,
    stage: &Path,
    per_file: u64,
    total: u64,
) -> Result<Resolved, Error> {
    let hash =
        hash32(digest).map_err(|_| Error::new(Stage::Association, Failure::InvalidReference))?;
    let catalog = catalog_path
        .map(|path| {
            super::resource::no_sidecars(path)
                .map_err(|_| Error::new(Stage::Discovery, Failure::UnsafeSource))?;
            let pin = Pin::open(path, false)
                .map_err(|_| Error::new(Stage::Discovery, Failure::UnsafeSource))?;
            let source = CatalogSource::from_path(path)
                .map_err(|_| Error::new(Stage::Discovery, Failure::IncompleteSources))?;
            let reference = source
                .find(&hash)
                .map_err(|error| {
                    if error.failure == Failure::NotFound {
                        Error::new(Stage::Discovery, Failure::IncompleteSources)
                    } else {
                        error
                    }
                })?
                .ok_or(Error::new(Stage::Association, Failure::NotFound))?;
            let material = source.material(&reference)?;
            if !material.md5.eq_ignore_ascii_case(&hash) {
                return Err(Error::new(Stage::Association, Failure::ConflictingEvidence));
            }
            Ok((source, reference, pin))
        })
        .transpose()?;
    let mut scan = Scan::new();
    let mut candidates = Vec::new();
    scan.walk(
        root,
        0,
        &|path| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(&hash))
        },
        &mut candidates,
    )
    .map_err(|_| Error::new(Stage::Discovery, Failure::UnsafeSource))?;
    if candidates.is_empty() {
        return Err(Error::new(Stage::Discovery, Failure::NotFound));
    }
    let mut remaining = total;
    let mut selected = None;
    for candidate in candidates {
        let bytes = bounded_read(&candidate, stage, per_file.min(remaining))
            .map_err(|_| Error::new(Stage::Discovery, Failure::Unavailable))?;
        remaining = remaining
            .checked_sub(bytes.len() as u64)
            .ok_or(Error::new(Stage::Discovery, Failure::LimitExceeded))?;
        if format!("{:x}", md5::compute(&bytes)) == hash {
            selected = Some(bytes);
            break;
        }
    }
    let bytes = selected.ok_or(Error::new(Stage::Association, Failure::ConflictingEvidence))?;
    scan.verify()
        .map_err(|_| Error::new(Stage::Revalidation, Failure::StaleEvidence))?;
    if let Some((source, reference, pin)) = &catalog {
        source.revalidate(reference)?;
        pin.verify()
            .map_err(|_| Error::new(Stage::Revalidation, Failure::StaleEvidence))?;
        super::resource::no_sidecars(catalog_path.expect("catalog path"))
            .map_err(|_| Error::new(Stage::Revalidation, Failure::StaleEvidence))?;
    }
    let format = decoder::detect_image_format(&bytes);
    if !matches!(format, "jpg" | "png" | "gif" | "webp") {
        return Err(Error::new(Stage::Decode, Failure::InvalidMaterial));
    }
    Ok(Resolved {
        bytes,
        format,
        binding: if catalog.is_some() {
            Binding::CatalogAndMessageDigest
        } else {
            Binding::MessageDigest
        },
    })
}
