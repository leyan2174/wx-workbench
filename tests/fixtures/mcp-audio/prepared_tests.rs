use super::*;
use serde_json::{json, Value};

fn voice() -> DatabaseVoice {
    DatabaseVoice {
        silk: b"\x02#!SILK_V3\0\xffsynthetic!".to_vec(),
        evidence: VoiceEvidence {
            username: "wxid_peer".into(),
            message_source: "message/message_2.db".into(),
            message_table: format!("Msg_{:x}", md5::compute("wxid_peer")),
            message_local_id: i64::MAX,
            server_id: i64::MIN,
            create_time: 0,
            media_source: "message/media_9.db".into(),
            media_rowid: 8,
            media_chat_name_id: 3,
            media_local_id: 700,
        },
    }
}

fn limits() -> Limits {
    Limits {
        max_audio_bytes: MAX_VOICE_BYTES,
        max_response_bytes: 24 * 1024 * 1024,
    }
}

#[test]
fn prepared_roundtrip_preserves_every_byte_and_i64_evidence() {
    let voice = voice();
    let payload = encode(&voice, limits()).unwrap();
    let value: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["silk_size_bytes"], voice.silk.len());
    assert_eq!(
        value["silk_sha256"],
        format!("{:x}", Sha256::digest(&voice.silk))
    );
    assert_eq!(decode(&payload, limits()).unwrap(), voice);
}

#[test]
fn prepared_audio_and_complete_json_limits_are_inclusive() {
    let voice = voice();
    let payload = encode(&voice, limits()).unwrap();
    let exact = Limits {
        max_audio_bytes: voice.silk.len(),
        max_response_bytes: payload.len(),
    };
    assert_eq!(encode(&voice, exact).unwrap(), payload);
    assert_eq!(decode(&payload, exact).unwrap(), voice);
    for smaller in [
        Limits {
            max_audio_bytes: voice.silk.len() - 1,
            ..exact
        },
        Limits {
            max_response_bytes: payload.len() - 1,
            ..exact
        },
    ] {
        assert!(encode(&voice, smaller).is_err());
        assert!(decode(&payload, smaller).is_err());
    }
    // base64 本身放得下，但加上证据和 JSON 包装仍必须拒绝。
    assert!(encode(
        &voice,
        Limits {
            max_response_bytes: encoded_len(voice.silk.len()),
            ..exact
        }
    )
    .is_err());
}

#[test]
fn prepared_hard_limit_is_sixteen_mib_even_when_caller_requests_more() {
    let mut voice = voice();
    voice.silk.resize(MAX_VOICE_BYTES, 0);
    let payload = encode(&voice, limits()).unwrap();
    assert_eq!(
        decode(&payload, limits()).unwrap().silk.len(),
        MAX_VOICE_BYTES
    );
    voice.silk.push(0);
    assert!(encode(&voice, limits()).is_err());
    for invalid in [
        Limits {
            max_audio_bytes: MAX_VOICE_BYTES + 1,
            ..limits()
        },
        Limits {
            max_audio_bytes: usize::MAX,
            ..limits()
        },
        Limits {
            max_audio_bytes: 0,
            ..limits()
        },
        Limits {
            max_response_bytes: 0,
            ..limits()
        },
    ] {
        assert!(encode(&voice, invalid).is_err());
        assert!(decode(b"{}", invalid).is_err());
    }
}

#[test]
fn prepared_corrupt_responses_never_reach_output_stage_or_echo_payload() {
    let original: Value = serde_json::from_slice(&encode(&voice(), limits()).unwrap()).unwrap();
    let mutations = [
        ("silk_base64", json!("SECRET-not-base64")),
        ("silk_sha256", json!("0".repeat(64))),
        ("silk_sha256", json!("A".repeat(64))),
        ("silk_size_bytes", json!(0)),
        ("silk_size_bytes", json!(u64::MAX)),
        ("silk_size_bytes", json!(-1)),
        ("silk_size_bytes", json!(1.5)),
        ("version", json!(2)),
        ("unknown", json!(true)),
    ];
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("must-not-exist.wav");
    for (key, value) in mutations {
        let mut changed = original.clone();
        changed[key] = value;
        let payload = serde_json::to_vec(&changed).unwrap();
        let result = decode(&payload, limits()).and_then(|v| {
            std::fs::write(&output, v.silk)?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!format!("{:#}", result.unwrap_err()).contains("SECRET"));
        assert!(!output.exists());
    }
}

#[test]
fn prepared_rejects_bad_evidence_unknown_fields_duplicates_and_trailing_json() {
    let original: Value = serde_json::from_slice(&encode(&voice(), limits()).unwrap()).unwrap();
    for (key, value) in [
        ("username", json!("other-account")),
        ("message_source", json!("../message_2.db")),
        ("media_source", json!("message/message_9.db")),
        ("message_local_id", json!(0)),
        ("server_id", json!(0)),
        ("extra", json!("SECRET")),
    ] {
        let mut changed = original.clone();
        changed["evidence"][key] = value;
        assert!(decode(&serde_json::to_vec(&changed).unwrap(), limits()).is_err());
    }
    let valid = serde_json::to_string(&original).unwrap();
    for bad in [
        format!("{valid} {{}}"),
        valid.replacen("{", "{\"version\":1,", 1),
        valid.replacen(
            "\"evidence\":{",
            "\"evidence\":{\"username\":\"wxid_peer\",",
            1,
        ),
        "null".into(),
        "[1,2,3]".into(),
    ] {
        assert!(decode(bad.as_bytes(), limits()).is_err());
    }
}

#[test]
fn prepared_checks_padding_decoded_size_hash_and_header_separately() {
    let original: Value = serde_json::from_slice(&encode(&voice(), limits()).unwrap()).unwrap();
    let base64 = original["silk_base64"].as_str().unwrap();
    for b64 in [
        format!("{base64}\n"),
        base64.trim_end_matches('=').into(),
        "!".repeat(base64.len()),
    ] {
        let mut changed = original.clone();
        changed["silk_base64"] = json!(b64);
        assert!(decode(&serde_json::to_vec(&changed).unwrap(), limits()).is_err());
    }
    let mut changed = original.clone();
    changed["silk_size_bytes"] = json!(voice().silk.len() + 1);
    assert!(decode(&serde_json::to_vec(&changed).unwrap(), limits()).is_err());
    let bytes = b"not-a-silk-stream";
    changed["silk_base64"] = json!(STANDARD.encode(bytes));
    changed["silk_sha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
    changed["silk_size_bytes"] = json!(bytes.len());
    assert!(decode(&serde_json::to_vec(&changed).unwrap(), limits()).is_err());
    let mut invalid = voice();
    invalid.silk = bytes.to_vec();
    assert!(encode(&invalid, limits()).is_err());
}
