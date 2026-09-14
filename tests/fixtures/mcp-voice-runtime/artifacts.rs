use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

pub const TIMESTAMP: i64 = 1_700_000_000;

pub fn silk(marker: &str, id: i64) -> Vec<u8> {
    let source = include_bytes!("../audio/silence.silk");
    let source = source.strip_prefix(&[2]).unwrap_or(source);
    assert!(source.starts_with(b"#!SILK_V3"));
    let packets = source[9..]
        .strip_suffix(&[255, 255])
        .unwrap_or(&source[9..]);
    let repeats = if marker == "A" { 1 } else { 3 } + usize::from(id == 701);
    let mut result = b"#!SILK_V3".to_vec();
    for _ in 0..repeats {
        result.extend_from_slice(packets);
    }
    result.extend_from_slice(&[255, 255]);
    result
}

pub fn wav(marker: &str, id: i64) -> Vec<u8> {
    let mut decoder = silk_codec::DecoderBuilder::default()
        .output_sample_rate(24_000)
        .build()
        .unwrap();
    let pcm = decoder.decode_bytes(&silk(marker, id)).unwrap();
    assert!(!pcm.is_empty());
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24_000u32.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&pcm);
    bytes
}

pub fn prepared(username: &str, marker: &str, id: i64) -> Value {
    let silk = silk(marker, id);
    json!({"version":1,"silk_base64":STANDARD.encode(&silk),
        "silk_sha256":format!("{:x}",Sha256::digest(&silk)),"silk_size_bytes":silk.len(),
        "evidence":{"username":username,"message_source":"message/message_0.db",
        "message_table":format!("Msg_{:x}",md5::compute(username)),"message_local_id":id-693,
        "server_id":id+9000,"create_time":TIMESTAMP+id-700,"media_source":"message/media_0.db",
        "media_rowid":id-699,"media_chat_name_id":7,"media_local_id":id}})
}

pub fn executable() -> &'static Path {
    static EXE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &EXE.get_or_init(|| {
        use std::os::windows::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fake whisper.exe");
        let mut command = Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/mcp-voice-runtime/fake.rs"),
            )
            .arg("-o")
            .arg(&path)
            .creation_flags(0x08000000);
        println!("COMMAND: {command:?}");
        let result = command.output().unwrap();
        println!(
            "FAKE COMPILE STDOUT: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        println!(
            "FAKE COMPILE STDERR: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.status.success());
        (root, path)
    })
    .1
}

pub fn model(root: &Path, marker: &str, id: i64, text: &str) -> PathBuf {
    let path = root.join(format!("model-{marker}-{id}.bin"));
    let expected = path.with_extension("wav");
    fs::write(&expected, wav(marker, id)).unwrap();
    fs::write(&path, format!("{}\n{text}", expected.display())).unwrap();
    path
}

pub fn decoded_path(text: &str) -> PathBuf {
    PathBuf::from(
        text.lines()
            .nth(1)
            .unwrap()
            .strip_prefix("  文件: ")
            .unwrap(),
    )
}

pub fn assert_decode(text: &str, marker: &str, id: i64, root: &Path) -> PathBuf {
    let path = decoded_path(text);
    assert!(path.starts_with(root), "{text}");
    let expected = wav(marker, id);
    assert_eq!(fs::read(&path).unwrap(), expected);
    let size = expected.len().to_string();
    let mut grouped = String::new();
    for (index, c) in size.chars().enumerate() {
        if index > 0 && (size.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    assert_eq!(
        text,
        format!(
            "解码成功!\n  文件: {}\n  时长: {:.1}秒\n  大小: {grouped} bytes",
            path.display(),
            (expected.len() - 44) as f64 / 48_000.0
        )
    );
    path
}

pub fn assert_transcribe(text: &str, result: &str, id: i64) {
    use chrono::{Local, TimeZone};
    let time = Local
        .timestamp_opt(TIMESTAMP + id - 700, 0)
        .unwrap()
        .format("%Y-%m-%d %H:%M");
    assert_eq!(text, format!("[{time}] (zh)\n{result}"));
}
