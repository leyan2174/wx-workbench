//! WeChat batch image layout and decoding policy. No filesystem or publication authority.
use crate::attachment::decoder::{self, DecodedImage, V2_MAGIC};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(crate) use decoder::V2KeyMaterial as KeyMaterial;
pub(crate) const INPUT_EXTENSION: &str = "dat";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Album,
    Mirror,
}

#[derive(Clone, Copy)]
pub(crate) enum DecodeMode {
    Single,
    Batch,
}

pub(crate) enum Decoded {
    Image(DecodedImage),
    MissingKey,
}

pub(crate) fn decode(bytes: &[u8], keys: KeyMaterial<'_>, mode: DecodeMode) -> Result<Decoded> {
    if matches!(mode, DecodeMode::Batch) && keys.aes_key.is_none() && bytes.starts_with(&V2_MAGIC) {
        return Ok(Decoded::MissingKey);
    }
    decoder::dispatch(bytes, keys).map(Decoded::Image)
}

fn stem(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .context("Missing image filename")?
        .to_string_lossy();
    Ok(stem
        .strip_suffix("_t")
        .or_else(|| stem.strip_suffix("_h"))
        .unwrap_or(&stem)
        .into())
}

pub(crate) fn with_format(base: &Path, extension: &str) -> PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(format!(".{extension}"));
    path.into()
}

pub(crate) fn single_output(input: &Path, format: &str) -> Result<PathBuf> {
    Ok(with_format(&input.with_file_name(stem(input)?), format))
}

pub(crate) fn default_input(database: &Path) -> Result<PathBuf> {
    Ok(super::legacy_dat::attach_root_for(
        database
            .parent()
            .context("Database has no account parent")?,
    ))
}

impl Layout {
    pub(crate) fn target(self, relative: &Path, output: &Path) -> Result<Option<PathBuf>> {
        let stem = stem(relative)?;
        Ok(Some(match self {
            Self::Album => {
                let parts: Vec<_> = relative.components().collect();
                if parts.len() != 4 || !parts[2].as_os_str().eq_ignore_ascii_case("Img") {
                    return Ok(None);
                }
                output.join(parts[0]).join(parts[1]).join(stem)
            }
            Self::Mirror => output.join(relative).with_file_name(stem),
        }))
    }

    pub(crate) fn existing_name(self, prefix: &str, name: &str) -> bool {
        let name = name.to_lowercase();
        name.starts_with(prefix) && !(self == Self::Album && name.ends_with(".tmp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_and_mirror_keep_historical_suffix_and_four_component_rules() {
        let output = Path::new("out");
        assert_eq!(
            Layout::Album
                .target(Path::new("chat/month/Img/photo.part_t.dat"), output)
                .unwrap(),
            Some(output.join("chat/month/photo.part"))
        );
        assert_eq!(
            Layout::Mirror
                .target(Path::new("nested/photo_h.dat"), output)
                .unwrap(),
            Some(output.join("nested/photo"))
        );
        assert_eq!(
            Layout::Album
                .target(Path::new("chat/month/Video/photo.dat"), output)
                .unwrap(),
            None
        );
        assert_eq!(
            Layout::Album
                .target(Path::new("extra/chat/month/Img/photo.dat"), output)
                .unwrap(),
            None
        );
        assert!(!Layout::Album.existing_name("photo.", "PHOTO.tmp"));
        assert!(Layout::Mirror.existing_name("photo.", "PHOTO.tmp"));
    }

    #[test]
    fn missing_key_skip_is_only_batch_policy_and_does_not_hide_decode_errors() {
        let keys = KeyMaterial {
            aes_key: None,
            xor_key: 0x88,
        };
        assert!(matches!(
            decode(&V2_MAGIC, keys, DecodeMode::Batch).unwrap(),
            Decoded::MissingKey
        ));
        assert!(decode(&V2_MAGIC, keys, DecodeMode::Single).is_err());
        assert!(decode(
            &V2_MAGIC,
            KeyMaterial::with_aes(b"synthetic-key-16"),
            DecodeMode::Batch
        )
        .is_err());
        let plain = b"\x89PNG\r\n\x1a\nsynthetic";
        let cipher: Vec<_> = plain.iter().map(|byte| byte ^ 0x37).collect();
        let Decoded::Image(image) = decode(&cipher, keys, DecodeMode::Batch).unwrap() else {
            panic!("legacy XOR is not a missing-key skip")
        };
        assert_eq!(image.data, plain);
    }
}
