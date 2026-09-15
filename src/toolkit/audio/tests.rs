use super::*;

pub(super) fn fixtures() -> PathBuf {
    std::env::var_os("WX_AUDIO_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // 同一源码也由 tests/fixtures 下的独立清单编译，不能假定清单就在仓库根。
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .map(|root| root.join("tests/fixtures/audio"))
                .find(|path| path.join("tone.silk").is_file())
                .expect("找不到仓库合成音频样本目录")
        })
}

fn packet(payload: &[u8]) -> Vec<u8> {
    let mut data = HEADER.to_vec();
    data.extend_from_slice(&(payload.len() as i16).to_le_bytes());
    data.extend_from_slice(payload);
    data
}

#[test]
fn normalizes_prefix_and_tail_without_mutating_source() {
    let plain = packet(&[1, 2, 3]);
    for prefix in [false, true] {
        for tail in [false, true] {
            let mut data = if prefix { vec![2] } else { vec![] };
            data.extend_from_slice(&plain);
            if tail {
                data.extend_from_slice(&[255, 255]);
            }
            let before = data.clone();
            let expected = [plain.as_slice(), &[255, 255]].concat();
            assert_eq!(normalize_silk(&data).unwrap(), expected);
            assert_eq!(data, before);
        }
    }
}

#[test]
fn payload_ffff_is_not_a_terminator() {
    let data = packet(&[255, 255]);
    assert_eq!(normalize_silk(&data).unwrap().len(), data.len() + 2);
}

#[test]
fn rejects_malformed_containers() {
    for data in [
        vec![],
        HEADER.to_vec(),
        b"\x02\x02#!SILK_V3".to_vec(),
        [HEADER, &[1]].concat(),
        [HEADER, &[4, 0, 1]].concat(),
        [HEADER, &[0, 0]].concat(),
        [HEADER, &[254, 255]].concat(),
        [packet(&[1]).as_slice(), &[255, 255, 0]].concat(),
    ] {
        assert!(normalize_silk(&data).is_err(), "{data:?}");
    }
    assert!(normalize_silk(&vec![0; MAX_INPUT_BYTES + 1]).is_err());
    let mut excessive = HEADER.to_vec();
    for _ in 0..=MAX_PACKETS {
        excessive.extend_from_slice(&[1, 0, 1]);
    }
    assert!(normalize_silk(&excessive).is_err());
}

#[test]
fn synthetic_sdk_pcm_matches_pilk() {
    // 独立验证与主仓集成共用同一组不含真实录音的 fixture。
    let fixture = fixtures();
    for stem in ["tone", "silence", "sweep", "multi40", "multi100"] {
        let silk = fs::read(fixture.join(format!("{stem}.silk"))).unwrap();
        let expected = fs::read(fixture.join(format!("{stem}.pcm"))).unwrap();
        assert_eq!(decode_silk_to_pcm(&silk).unwrap(), expected, "{stem}");
        let normalized = normalize_silk(&silk).unwrap();
        assert_eq!(
            decode_silk_to_pcm(&normalized[..normalized.len() - 2]).unwrap(),
            expected
        );
    }
}

#[test]
fn missing_or_failed_encoder_preserves_target_and_cleans_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.silk");
    let original = fs::read(fixtures().join("tone.silk")).unwrap();
    fs::write(&source, &original).unwrap();
    let target = temp.path().join("target.mp3");
    fs::write(&target, b"old mp3").unwrap();
    // 测试程序无法识别 ffmpeg 参数，将真实返回非零退出码。
    for executable in [
        temp.path().join("missing.exe"),
        std::env::current_exe().unwrap(),
    ] {
        assert!(convert_silk_to_mp3_with_ffmpeg(&source, &target, &executable).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old mp3");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

#[test]
#[ignore = "requires native ffmpeg and ffprobe in PATH; run explicitly for audio integration"]
fn mp3_matches_legacy_pcm_pipeline_and_has_expected_format() {
    let temp = tempfile::tempdir().unwrap();
    for stem in ["tone", "silence", "sweep", "multi40", "multi100"] {
        let source = fixtures().join(format!("{stem}.silk"));
        let before = fs::read(&source).unwrap();
        let output = temp.path().join(format!("{stem}.mp3"));
        fs::write(&output, b"previous output").unwrap();
        let converted = convert_silk_to_mp3(&source, &output).unwrap();
        let reference = temp.path().join(format!("{stem}-reference.mp3"));
        let result = hidden(
            Command::new("ffmpeg")
                .args(["-y", "-f", "s16le", "-ar", "24000", "-ac", "1", "-i"])
                .arg(fixtures().join(format!("{stem}.pcm")))
                .arg(&reference),
        )
        .output()
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read(&output).unwrap(),
            fs::read(&reference).unwrap(),
            "{stem}"
        );
        assert_eq!(converted.size, fs::metadata(&output).unwrap().len());
        assert_eq!(fs::read(&source).unwrap(), before);
        let probe = hidden(
            Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_name,sample_rate,channels",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&output),
        )
        .output()
        .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let properties = String::from_utf8(probe.stdout).unwrap();
        assert!(properties.lines().any(|line| line == "codec_name=mp3"));
        assert!(properties.lines().any(|line| line == "sample_rate=24000"));
        assert!(properties.lines().any(|line| line == "channels=1"));
        println!(
            "{stem}: PCM byte parity, MP3 byte parity, {} bytes, 24000 Hz mono",
            converted.size
        );
    }
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 10);
}

#[test]
fn rejects_source_alias_and_preserves_outputs_on_failure() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("input.silk");
    fs::write(&source, b"not silk").unwrap();
    assert!(convert_silk_to_mp3(&source, &source).is_err());
    let alias = temp.path().join("alias.mp3");
    fs::hard_link(&source, &alias).unwrap();
    assert!(convert_silk_to_mp3(&source, &alias).is_err());
    let target = temp.path().join("out.mp3");
    fs::write(&target, b"existing output").unwrap();
    assert!(convert_silk_to_mp3(&source, &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"existing output");
    assert_eq!(fs::read(&source).unwrap(), b"not silk");
}

#[test]
fn checked_conversion_rejects_protected_output_before_decoding() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("input.silk");
    let protected = temp.path().join("keys.synthetic");
    fs::write(&source, b"not silk").unwrap();
    fs::write(&protected, b"synthetic protected material").unwrap();
    let error = convert_silk_to_mp3_checked(
        &source,
        &protected,
        std::slice::from_ref(&protected),
        || Ok(()),
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains("SILK"), "{error:#}");
    assert_eq!(
        fs::read(&protected).unwrap(),
        b"synthetic protected material"
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}
