//! 恢复 `.dat` 中的标准图片文件字节，不进行像素解码或重新编码。
//!
//! 三档：
//! | header[0..6]            | decoder           | 备注                                    |
//! |-------------------------|-------------------|-----------------------------------------|
//! | `07 08 V2 08 07`        | `v2`              | AES-128-ECB + XOR 混合，需要 image AES key |
//! | `07 08 V1 08 07`        | `v1_aes`          | 固定 AES key `cfcd208495d565ef`         |
//! | (其他, 通常无 magic)    | `v1_xor`          | legacy single-byte XOR，magic 自动探测  |
//!
//! 决策点放在 `restore`，上层不重复解释 DAT 包装或密码规则。

use anyhow::{anyhow, Result};

pub mod v1_xor;
pub mod v2;

/// 完整 V2 magic：`\x07\x08V2\x08\x07`
pub const V2_MAGIC: [u8; 6] = [0x07, 0x08, b'V', b'2', 0x08, 0x07];
/// 完整 V1 magic：`\x07\x08V1\x08\x07`
pub const V1_MAGIC: [u8; 6] = [0x07, 0x08, b'V', b'1', 0x08, 0x07];

/// 恢复后的文件字节及识别格式，不是解码后的像素。
#[derive(Debug)]
pub struct RestoredImage {
    pub data: Vec<u8>,
    /// 推断出的图片扩展名（不带点），由 magic 决定。例如 "jpg" / "png" / "gif" / "webp" /
    /// "tif" / "bmp" / "hevc"（wxgf 容器）/ "bin"（未识别）
    pub format: &'static str,
    /// 解码器名称（"legacy_xor" / "v1_aes" / "v2"），用于 CLI 调试输出
    pub decoder: &'static str,
}

/// 由 caller 提供的 V2 image AES key（codex 的 `image_key` 模块负责拿到）。
/// 缺省时遇到 V2 文件会返回 `Err`，caller 可以拿到具体错误信息再处理。
#[derive(Clone, Copy)]
pub struct V2KeyMaterial<'a> {
    pub aes_key: Option<&'a [u8; 16]>,
    /// XOR key — WeChat 4.x 默认 0x88，可 override
    pub xor_key: u8,
}

impl std::fmt::Debug for V2KeyMaterial<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("V2KeyMaterial([REDACTED])")
    }
}

impl Default for V2KeyMaterial<'_> {
    fn default() -> Self {
        Self {
            aes_key: None,
            xor_key: 0x88,
        }
    }
}

impl<'a> V2KeyMaterial<'a> {
    pub fn with_aes(key: &'a [u8; 16]) -> Self {
        Self {
            aes_key: Some(key),
            ..Self::default()
        }
    }
}

/// 根据 `dat_bytes` 头部 magic 自动分发到对应 decoder。
///
/// AES 参数仅用于 V2；XOR 参数同时用于 V1 和 V2，显式账号值不能被默认值覆盖。
pub fn restore(dat_bytes: &[u8], v2_key: V2KeyMaterial<'_>) -> Result<RestoredImage> {
    if dat_bytes.len() >= 6 {
        let head: &[u8; 6] = dat_bytes[..6].try_into().unwrap();
        if head == &V2_MAGIC {
            return v2::restore(dat_bytes, v2_key);
        }
        if head == &V1_MAGIC {
            // V1 fixed-AES: 固定 key = md5("0")[:16] = "cfcd208495d565ef"
            let fixed_key: [u8; 16] = *b"cfcd208495d565ef";
            return v2::restore(
                dat_bytes,
                V2KeyMaterial {
                    aes_key: Some(&fixed_key),
                    xor_key: v2_key.xor_key,
                },
            )
            .map(|mut d| {
                d.decoder = "v1_aes";
                d
            });
        }
    }
    if dat_bytes.is_empty() {
        return Err(anyhow!("空 .dat 文件"));
    }
    v1_xor::restore(dat_bytes)
}

/// 从解密后的字节流头部探测图片格式扩展名。
///
/// 与上游 `decode_image.py::detect_image_format` 一致；新增 wxgf (HEVC 裸流) 的探测，
/// 因为 V2 解码后产物可能直接是 wxgf 容器。
pub fn detect_image_format(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 4 && &bytes[..4] == b"wxgf" {
        return "hevc";
    }
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        return "jpg";
    }
    if bytes.len() >= 4 && bytes[..4] == [0x89, 0x50, 0x4E, 0x47] {
        return "png";
    }
    if bytes.len() >= 3 && &bytes[..3] == b"GIF" {
        return "gif";
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    if bytes.len() >= 4 && bytes[..4] == [0x49, 0x49, 0x2A, 0x00] {
        return "tif";
    }
    if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        return "bmp";
    }
    "bin"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_default_xor_and_explicit_account_values_decode_exact_bytes() {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        assert!(V2KeyMaterial::default().aes_key.is_none());
        assert_eq!(V2KeyMaterial::default().xor_key, 0x88);
        let mut padded = [13u8; 16];
        padded[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
        let mut block = GenericArray::clone_from_slice(&padded);
        aes::Aes128::new(b"cfcd208495d565ef".into()).encrypt_block(&mut block);
        let mut expected = vec![0xff, 0xd8, 0xff];
        expected.extend_from_slice(b"synthetic raw segment");
        expected.extend_from_slice(&[0xff, 0xd9]);
        for xor_key in [0x88u8, 0x00, 0xa2] {
            let mut bytes = V1_MAGIC.to_vec();
            bytes.extend_from_slice(&3u32.to_le_bytes());
            bytes.extend_from_slice(&2u32.to_le_bytes());
            bytes.push(0);
            bytes.extend_from_slice(&block);
            bytes.extend_from_slice(b"synthetic raw segment");
            bytes.extend_from_slice(&[0xff ^ xor_key, 0xd9 ^ xor_key]);
            let key = if xor_key == 0x88 {
                V2KeyMaterial::default()
            } else {
                V2KeyMaterial {
                    xor_key,
                    ..V2KeyMaterial::default()
                }
            };
            let decoded = restore(&bytes, key).unwrap();
            assert_eq!(decoded.data, expected);
            assert_eq!(decoded.format, "jpg");
            assert_eq!(decoded.decoder, "v1_aes");
        }
    }

    #[test]
    fn detect_basic_formats() {
        assert_eq!(detect_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(detect_image_format(&[0x89, 0x50, 0x4E, 0x47]), "png");
        assert_eq!(detect_image_format(b"GIF89a"), "gif");
        assert_eq!(detect_image_format(b"BM\0\0\0\0\0\0\0\0\0\0\0\0"), "bmp");
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(&[0; 4]);
        assert_eq!(detect_image_format(&webp), "webp");
        assert_eq!(detect_image_format(&[0x49, 0x49, 0x2A, 0x00]), "tif");
        assert_eq!(detect_image_format(b"wxgfXXXX"), "hevc");
        assert_eq!(detect_image_format(&[0, 0, 0, 0]), "bin");
    }

    #[test]
    fn image_material_debug_never_includes_password_bytes() {
        let aes = *b"syntheticAESkey1";
        let material = V2KeyMaterial {
            aes_key: Some(&aes),
            xor_key: 0xa2,
        };
        assert_eq!(format!("{material:?}"), "V2KeyMaterial([REDACTED])");
        assert_eq!(
            format!("{:?}", V2KeyMaterial::default()),
            "V2KeyMaterial([REDACTED])"
        );
    }
}
