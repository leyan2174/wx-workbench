use crate::{asr, audio::publish::publish_wav_noclobber, local_files::HostOutputGuard};
use sha2::{Digest, Sha256};
use std::{cell::Cell, fs, path::Path};

fn empty(root: &Path) {
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

fn wav() -> Vec<u8> {
    asr::pcm24k_to_wav(&[0, 0, 255, 127, 0, 128]).unwrap()
}

fn target(root: &Path, bytes: &[u8]) -> std::path::PathBuf {
    root.join(format!("{:x}.wav", Sha256::digest(bytes)))
}

fn with_extra_chunk() -> Vec<u8> {
    let original = wav();
    let mut bytes = original[..12].to_vec();
    bytes.extend_from_slice(b"JUNK\x03\0\0\0abc\0");
    bytes.extend_from_slice(&original[12..]);
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    bytes
}

pub fn run_suite() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let guard = HostOutputGuard::new(&output).unwrap();
    let wav = wav();
    let mut response = None;
    let result = publish_wav_noclobber(&wav, &guard, |description| {
        assert!(!description.path.exists());
        assert_eq!(description.path, target(&output, &wav));
        assert_eq!(description.size, wav.len() as u64);
        assert_eq!(description.pcm_bytes, 6);
        assert_eq!(description.sample_rate, 24_000);
        response = Some(description.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(response.unwrap(), result);
    assert_eq!(fs::read(&result.path).unwrap(), wav);
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    assert!(publish_wav_noclobber(&wav, &guard, |_| Ok(())).is_err());
    assert_eq!(fs::read(&result.path).unwrap(), wav);
    fs::remove_file(&result.path).unwrap();
    empty(&output);
    println!("PASS complete bytes, SHA256 name, borrowed response before commit, repeat refused");

    let bytes = with_extra_chunk();
    assert_eq!(asr::prepare_wav_bytes(&bytes).unwrap(), bytes);
    let info = asr::validate_wav(&bytes).unwrap();
    assert_eq!(info.pcm_bytes, 6);
    assert_ne!(info.pcm_bytes, (bytes.len() - 44) as u64);
    let result = publish_wav_noclobber(&bytes, &guard, |_| Ok(())).unwrap();
    assert_eq!(result.pcm_bytes, 6);
    fs::remove_file(result.path).unwrap();
    let mut bytes = wav.clone();
    bytes[24..28].copy_from_slice(&16_000u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&32_000u32.to_le_bytes());
    let result = publish_wav_noclobber(&bytes, &guard, |_| Ok(())).unwrap();
    assert_eq!(result.sample_rate, 16_000);
    assert_eq!(result.pcm_bytes, 6);
    fs::remove_file(result.path).unwrap();
    println!("PASS padded ancillary chunk and non-24kHz metadata, prepare_wav_bytes unchanged");

    let silk = include_bytes!("../audio/silence.silk");
    let prepared = asr::prepare_wav_bytes(silk).unwrap();
    let result = publish_wav_noclobber(&prepared, &guard, |_| Ok(())).unwrap();
    let pcm = crate::audio::decode_silk_to_pcm(silk).unwrap();
    assert_eq!(result.pcm_bytes, pcm.len() as u64);
    assert_eq!(result.sample_rate, 24_000);
    assert_eq!(fs::read(&result.path).unwrap(), prepared);
    fs::remove_file(result.path).unwrap();
    println!("PASS existing SILK decoder and prepare_wav_bytes feed real publisher");

    assert!(
        publish_wav_noclobber(&wav, &guard, |_| anyhow::bail!("cancelled before commit")).is_err()
    );
    empty(&output);
    println!("PASS callback rejection leaves no final or temporary file");

    let path = target(&output, &wav);
    fs::write(&path, b"existing").unwrap();
    assert!(publish_wav_noclobber(&wav, &guard, |_| Ok(())).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"existing");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(publish_wav_noclobber(&wav, &guard, |_| Ok(())).is_err());
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"hardlink-source").unwrap();
    fs::hard_link(&source, &path).unwrap();
    assert!(publish_wav_noclobber(&wav, &guard, |_| Ok(())).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"hardlink-source");
    assert_eq!(fs::read(&path).unwrap(), b"hardlink-source");
    fs::remove_file(&path).unwrap();
    empty(&output);
    println!("PASS existing file/directory/hardlink never overwritten");

    assert!(publish_wav_noclobber(&wav, &guard, |description| {
        fs::write(&description.path, b"won-race")?;
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(&path).unwrap(), b"won-race");
    fs::remove_file(&path).unwrap();
    println!("PASS target created in callback wins without overwrite");

    assert!(publish_wav_noclobber(&wav, &guard, |_| {
        let staged = fs::read_dir(&output)?.next().unwrap()?.path();
        fs::write(staged, vec![0u8; wav.len()])?;
        Ok(())
    })
    .is_err());
    empty(&output);
    println!("PASS temporary content changed in callback rejected");

    let called = Cell::new(false);
    let mut malformed = vec![Vec::new(), b"#!SILK_V3".to_vec(), wav[..20].to_vec()];
    for index in [0, 4, 8, 20, 22, 28, 32, 34, 40] {
        let mut bytes = wav.clone();
        bytes[index] = 255;
        malformed.push(bytes);
    }
    let mut duplicate = wav.clone();
    duplicate.extend_from_slice(&wav[36..]);
    let size = (duplicate.len() - 8) as u32;
    duplicate[4..8].copy_from_slice(&size.to_le_bytes());
    malformed.push(duplicate);
    malformed.push(vec![0; 32 * 1024 * 1024 + 1]);
    for bytes in malformed {
        assert!(publish_wav_noclobber(&bytes, &guard, |_| {
            called.set(true);
            Ok(())
        })
        .is_err());
        empty(&output);
    }
    assert!(!called.get());
    println!(
        "PASS malformed, duplicate data, SILK and oversized WAV refused before callback/staging"
    );

    let mut protected = HostOutputGuard::new(&output).unwrap();
    assert!(protected.protect(&output.join("protected.db")).is_err());
    assert!(HostOutputGuard::new(Path::new("relative")).is_err());
    assert!(HostOutputGuard::new(&root.path().join("missing")).is_err());
    protected.pin_input(&source).unwrap();
    let failed = publish_wav_noclobber(&wav, &protected, |_| {
        fs::write(&source, b"changed")?;
        Ok(())
    });
    assert!(failed.is_err());
    empty(&output);
    println!("PASS shared guard rejects overlap, missing/relative root and pinned-source change");

    let race_root = tempfile::tempdir().unwrap();
    let race_output = race_root.path().join("output");
    let moved = race_root.path().join("moved");
    fs::create_dir(&race_output).unwrap();
    let race_guard = HostOutputGuard::new(&race_output).unwrap();
    assert!(publish_wav_noclobber(&wav, &race_guard, |_| {
        fs::rename(&race_output, &moved)?;
        fs::create_dir(&race_output)?;
        Ok(())
    })
    .is_err());
    assert!(!target(&race_output, &wav).exists());
    assert!(!target(&moved, &wav).exists());
    println!("PASS output directory rename/replacement in callback cannot publish");

    #[cfg(windows)]
    {
        let link = std::os::windows::fs::symlink_file(&source, &path);
        match link {
            Ok(()) => {
                assert!(publish_wav_noclobber(&wav, &guard, |_| Ok(())).is_err());
                assert!(fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                fs::remove_file(&path).unwrap();
                println!("PASS existing symbolic link never overwritten");
            }
            Err(error) if error.raw_os_error() == Some(1314) => {
                println!("UNVERIFIED symbolic-link case: Windows privilege not available");
            }
            Err(error) => panic!("create fixture symbolic link: {error}"),
        }
    }
}
