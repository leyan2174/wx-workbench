use super::*;
#[cfg(feature = "sns-wasm-test-asset")]
fn asset() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = if manifest.join("src/adapters/wechat/media").is_dir() {
        manifest
    } else {
        manifest.join("../../..")
    };
    root.join("src/adapters/wechat/media/assets/wasm_video_decode.wasm")
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn node_vectors_byte_exact() {
    let runtime = SnsKeystream::new(&asset(), RuntimeLimits::default()).unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/sns-video-native/vectors.json"
    ))
    .unwrap();
    for v in vectors.as_array().unwrap() {
        let hex = v["hex"].as_str().unwrap();
        let expected: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|p| u8::from_str_radix(&hex[p..p + 2], 16).unwrap())
            .collect();
        let actual = runtime
            .keystream(
                v["key"].as_str().unwrap(),
                v["size"].as_u64().unwrap() as usize,
            )
            .unwrap();
        assert_eq!(actual, expected, "synthetic vector id {}", v["id"]);
    }
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn decode_prefix_and_plaintext() {
    let runtime = SnsKeystream::new(&asset(), RuntimeLimits::default()).unwrap();
    for size in [
        12,
        15,
        16,
        VIDEO_PREFIX_BYTES - 1,
        VIDEO_PREFIX_BYTES,
        VIDEO_PREFIX_BYTES + 29,
    ] {
        let mut plain: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        plain[4..8].copy_from_slice(b"ftyp");
        let mut encrypted = plain.clone();
        let stream = runtime
            .keystream("42", size.min(VIDEO_PREFIX_BYTES))
            .unwrap();
        for (b, k) in encrypted.iter_mut().zip(stream) {
            *b ^= k;
        }
        assert_eq!(runtime.restore_video("42", &encrypted).unwrap(), plain);
        assert_eq!(runtime.restore_video("", &plain).unwrap(), plain);
    }
    assert_eq!(
        runtime.restore_video("42", &[0; 15]),
        Err(KeystreamError::InvalidMp4)
    );
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn rejects_inputs_and_resource_exhaustion() {
    let runtime = SnsKeystream::new(&asset(), RuntimeLimits::default()).unwrap();
    for (key, size) in [
        ("", 8),
        (" \t", 8),
        ("1", 0),
        ("1", MAX_KEYSTREAM_BYTES + 1),
        ("1", usize::MAX),
    ] {
        assert_eq!(
            runtime.keystream(key, size),
            Err(KeystreamError::InvalidInput)
        );
    }
    assert_eq!(
        runtime.keystream(&"1".repeat(KEY_MAX_BYTES + 1), 8),
        Err(KeystreamError::InvalidInput)
    );
    assert_eq!(
        runtime.restore_video("1", &[]),
        Err(KeystreamError::InvalidInput)
    );
    for limits in [
        RuntimeLimits {
            fuel: 1,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            memory_bytes: 65536,
            ..RuntimeLimits::default()
        },
    ] {
        let bounded = SnsKeystream::new(&asset(), limits).unwrap();
        assert_eq!(bounded.keystream("1", 8), Err(KeystreamError::Runtime));
    }
    let bounded = SnsKeystream::new(
        &asset(),
        RuntimeLimits {
            input_bytes: 4,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        bounded.restore_video("1", &[0; 5]),
        Err(KeystreamError::InvalidInput)
    );
    assert_eq!(bounded.keystream("1", 5), Err(KeystreamError::InvalidInput));
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn large_node_oracles_and_finite_default_fuel() {
    let runtime = SnsKeystream::bundled(RuntimeLimits::default()).unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/sns-video-native/large-vectors.json"
    ))
    .unwrap();
    for v in vectors.as_array().unwrap() {
        let size = v["size"].as_u64().unwrap() as usize;
        let actual = runtime.keystream(v["key"].as_str().unwrap(), size);
        if size > MAX_KEYSTREAM_BYTES {
            // Node 可以生成此长度，但图片业务上限必须在启动 WASM 前拒绝。
            assert_eq!(actual, Err(KeystreamError::InvalidInput));
        } else {
            let bytes = actual.unwrap();
            assert_eq!(bytes.len(), size);
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                v["sha256"].as_str().unwrap()
            );
        }
    }
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn callback_alignment_and_memory_bounds() {
    assert_eq!(capture_range(16, 8, 8, 24).unwrap(), 16..24);
    assert_eq!(
        capture_range(
            0,
            MAX_KEYSTREAM_BYTES,
            MAX_KEYSTREAM_BYTES,
            MAX_KEYSTREAM_BYTES
        )
        .unwrap(),
        0..MAX_KEYSTREAM_BYTES
    );
    for (start, n, expected, memory) in [
        (0, 0, 0, 8),
        (0, 7, 7, 8),
        (0, 8, 16, 16),
        (
            0,
            MAX_KEYSTREAM_BYTES + 8,
            MAX_KEYSTREAM_BYTES + 8,
            usize::MAX,
        ),
        (17, 8, 8, 24),
        (usize::MAX - 3, 8, 8, usize::MAX),
    ] {
        assert!(capture_range(start, n, expected, memory).is_err());
    }
    // input_bytes 限制原请求长度；guest 仅允许必要的最多 7 字节对齐填充。
    let runtime = SnsKeystream::bundled(RuntimeLimits {
        input_bytes: 9,
        ..RuntimeLimits::default()
    })
    .unwrap();
    assert_eq!(runtime.keystream("42", 9).unwrap().len(), 9);
    assert_eq!(
        runtime.keystream("42", 10),
        Err(KeystreamError::InvalidInput)
    );
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn fuel_budget_is_bounded_and_respects_caller() {
    assert_eq!(keystream_fuel(8, u64::MAX), BASE_FUEL);
    assert_eq!(keystream_fuel(VIDEO_PREFIX_BYTES, u64::MAX), BASE_FUEL);
    assert_eq!(
        keystream_fuel(VIDEO_PREFIX_BYTES + 8, u64::MAX),
        BASE_FUEL + 64
    );
    assert_eq!(keystream_fuel(MAX_KEYSTREAM_BYTES, u64::MAX), MAX_FUEL);
    assert_eq!(keystream_fuel(usize::MAX, u64::MAX), MAX_FUEL);
    for size in [8, VIDEO_PREFIX_BYTES + 8, MAX_KEYSTREAM_BYTES] {
        assert_eq!(keystream_fuel(size, 0), 0);
        assert_eq!(keystream_fuel(size, 1), 1);
    }
    for limits in [
        RuntimeLimits {
            fuel: 1,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            memory_bytes: 65536,
            ..RuntimeLimits::default()
        },
    ] {
        let runtime = SnsKeystream::bundled(limits).unwrap();
        assert_eq!(
            runtime.keystream("42", MAX_KEYSTREAM_BYTES),
            Err(KeystreamError::Runtime)
        );
    }
    let runtime = SnsKeystream::bundled(RuntimeLimits {
        fuel: BASE_FUEL,
        ..RuntimeLimits::default()
    })
    .unwrap();
    assert_eq!(
        runtime.keystream("42", MAX_KEYSTREAM_BYTES),
        Err(KeystreamError::Runtime)
    );
    // 一次燃料耗尽不污染下一次隔离 Store。
    assert_eq!(runtime.keystream("42", 8).unwrap().len(), 8);
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn video_still_decrypts_only_128k_prefix() {
    let runtime = SnsKeystream::bundled(RuntimeLimits::default()).unwrap();
    let mut plain = vec![0xa5; VIDEO_PREFIX_BYTES * 2 + 1];
    plain[4..8].copy_from_slice(b"ftyp");
    let mut encrypted = plain.clone();
    let stream = runtime.keystream("42", VIDEO_PREFIX_BYTES).unwrap();
    for (byte, mask) in encrypted.iter_mut().zip(stream) {
        *byte ^= mask;
    }
    assert_eq!(runtime.restore_video("42", &encrypted).unwrap(), plain);
    assert_eq!(
        &encrypted[VIDEO_PREFIX_BYTES..],
        &plain[VIDEO_PREFIX_BYTES..]
    );
    // 视频输入仍可超过图片密钥流上限，不能误用通用上限截断整个视频。
    let mut large_plain = vec![0; MAX_KEYSTREAM_BYTES + 1];
    large_plain[4..8].copy_from_slice(b"ftyp");
    assert_eq!(
        runtime.restore_video("", &large_plain).unwrap(),
        large_plain
    );
}

#[test]
fn rejects_unknown_or_missing_assets() {
    #[cfg(not(feature = "sns-wasm-test-asset"))]
    assert!(matches!(
        SnsKeystream::bundled(RuntimeLimits::default()),
        Err(KeystreamError::AssetUnavailable)
    ));
    #[cfg(feature = "sns-wasm-test-asset")]
    assert!(SnsKeystream::bundled(RuntimeLimits::default()).is_ok());
    assert!(matches!(
        SnsKeystream::new(
            Path::new("sns-video-native-missing.wasm"),
            RuntimeLimits::default()
        ),
        Err(KeystreamError::AssetRead)
    ));
    let directory = tempfile::tempdir().unwrap();
    let text_asset = directory.path().join("not-wasm.txt");
    std::fs::write(&text_asset, b"synthetic non-WASM asset").unwrap();
    assert!(matches!(
        SnsKeystream::new(&text_asset, RuntimeLimits::default()),
        Err(KeystreamError::UnsupportedAsset)
    ));
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn invalid_key_is_redacted_and_store_is_isolated() {
    let runtime = SnsKeystream::new(&asset(), RuntimeLimits::default()).unwrap();
    let first = runtime.keystream("1", 16).unwrap();
    let synthetic_bad_key = "synthetic-not-a-number";
    let error = runtime.keystream(synthetic_bad_key, 16).unwrap_err();
    assert!(!format!("{error:?}: {error}").contains(synthetic_bad_key));
    assert_eq!(error, KeystreamError::Runtime);
    assert_ne!(runtime.keystream("2", 16).unwrap(), first);
    assert_eq!(runtime.keystream("1", 16).unwrap(), first);
    assert_eq!(runtime.keystream("\u{feff}1\u{feff}", 16).unwrap(), first);
    let bounded = SnsKeystream::new(
        &asset(),
        RuntimeLimits {
            fuel: 100_000,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        bounded.keystream("1", VIDEO_PREFIX_BYTES),
        Err(KeystreamError::Runtime)
    );
}
