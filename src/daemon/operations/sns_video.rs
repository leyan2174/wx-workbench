//! 离线视频流式发布；仅在头部解码成功后创建临时文件，绝不覆盖已有结果。
use crate::toolkit::sns::video_runtime::{RuntimeLimits, VideoRuntime, VIDEO_PREFIX_BYTES};
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub fn cmd_decode(
    input: PathBuf,
    output: PathBuf,
    key_file: Option<PathBuf>,
    wasm: Option<PathBuf>,
) -> Result<()> {
    let bytes = decode_file(&input, &output, key_file.as_deref(), wasm.as_deref())?;
    println!(
        "{}",
        serde_json::json!({"engine": "rust-wasmi", "output": output, "bytes": bytes})
    );
    Ok(())
}

fn decode_file(
    input: &Path,
    output: &Path,
    key_file: Option<&Path>,
    wasm: Option<&Path>,
) -> Result<u64> {
    match fs::symlink_metadata(output) {
        Ok(_) => anyhow::bail!("输出已存在，拒绝覆盖"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("无法检查输出路径"),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // 保持源句柄直到发布完成；Windows 下禁止其他句柄写入或删除源文件。
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut source = options.open(input).context("无法打开输入视频")?;
    let metadata = source.metadata()?;
    ensure!(metadata.is_file(), "输入必须是普通文件");
    let mut prefix = Zeroizing::new(Vec::new());
    (&mut source)
        .take(VIDEO_PREFIX_BYTES as u64)
        .read_to_end(&mut prefix)?;
    let plaintext = prefix.len() >= 12 && &prefix[4..8] == b"ftyp";
    if !plaintext {
        let key_path = key_file.context("加密视频需要 --key-file")?;
        let mut key = Zeroizing::new(Vec::new());
        fs::File::open(key_path)
            .context("无法打开视频密钥文件")?
            .take(1025)
            .read_to_end(&mut key)
            .context("无法读取视频密钥文件")?;
        ensure!(key.len() <= 1024, "视频密钥文件超过长度限制");
        let key_text = std::str::from_utf8(&key).context("视频密钥文件必须是 UTF-8 文本")?;
        let runtime = match wasm {
            Some(path) => VideoRuntime::new(path, RuntimeLimits::default())?,
            None => VideoRuntime::bundled(RuntimeLimits::default())?,
        };
        prefix = Zeroizing::new(runtime.decode(key_text, &prefix)?);
    }
    let mut protected = vec![input.to_path_buf()];
    if !plaintext {
        protected.extend(key_file.map(Path::to_path_buf));
        protected.extend(wasm.map(Path::to_path_buf));
    }
    let target = crate::toolkit::ExportTarget::new_file(output, &protected)?;
    let source_identity = same_file::Handle::from_file(source.try_clone()?)?;
    let source_guard = source.try_clone()?;
    let mut total = 0;
    target
        .write_with_checked(
            |temporary| {
                let mut temporary = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(temporary)?;
                temporary.write_all(&prefix)?;
                total = prefix.len() as u64 + std::io::copy(&mut source, &mut temporary)?;
                ensure!(total == metadata.len(), "输入视频在读取过程中发生变化");
                Ok(())
            },
            || {
                ensure!(
                    source_guard.metadata()?.len() == metadata.len()
                        && same_file::Handle::from_path(input)? == source_identity,
                    "输入视频在发布前发生变化"
                );
                Ok(())
            },
        )
        .context("无法发布视频；已有输出不会被覆盖")?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_encrypted_prefix_and_preserves_tail() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.bin");
        let output = dir.path().join("result/video.mp4");
        let key = dir.path().join("key.txt");
        fs::write(&key, "42\n").unwrap();
        let mut plain = vec![23u8; VIDEO_PREFIX_BYTES + 12345];
        plain[4..8].copy_from_slice(b"ftyp");
        let mut encrypted = plain.clone();
        let runtime = VideoRuntime::bundled(RuntimeLimits::default()).unwrap();
        for (b, k) in encrypted
            .iter_mut()
            .zip(runtime.keystream("42", VIDEO_PREFIX_BYTES).unwrap())
        {
            *b ^= k;
        }
        fs::write(&input, &encrypted).unwrap();
        assert_eq!(
            decode_file(&input, &output, Some(&key), None).unwrap(),
            plain.len() as u64
        );
        assert_eq!(fs::read(&output).unwrap(), plain);
        assert_eq!(fs::read(&input).unwrap(), encrypted);
        assert!(decode_file(&input, &output, Some(&key), None).is_err());
        assert_eq!(fs::read(&output).unwrap(), plain);
        assert!(decode_file(&input, &input, Some(&key), None).is_err());
    }

    #[test]
    fn plaintext_needs_no_key_and_failures_publish_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.bin");
        let output = dir.path().join("result/video.mp4");
        let key = dir.path().join("key.txt");
        fs::write(&input, [0u8; 32]).unwrap();
        assert!(decode_file(&input, &output, None, None).is_err());
        fs::write(&key, "42").unwrap();
        assert!(decode_file(&input, &output, Some(&key), None).is_err());
        fs::write(&key, vec![b'1'; 1025]).unwrap();
        assert!(decode_file(&input, &output, Some(&key), None).is_err());
        assert!(!output.parent().unwrap().exists());
        fs::write(&input, b"\0\0\0\x0cftypisom").unwrap();
        assert_eq!(decode_file(&input, &output, None, None).unwrap(), 12);
        assert_eq!(fs::read(&output).unwrap(), fs::read(&input).unwrap());
    }
}
