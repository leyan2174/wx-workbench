//! 本地 IPC 的准备语音字节桥；只处理内存，不写音频、不调用后端、不代表公开工具成功。
use super::database_media::{DatabaseVoice, VoiceEvidence, MAX_VOICE_BYTES};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Write};

/// 调用方明确提供两种上限；响应上限覆盖准备语音 JSON，不包含外层 IPC 包装。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_audio_bytes: usize,
    pub max_response_bytes: usize,
}

impl Limits {
    /// 拒绝无效配置而非静默放宽；音频硬上限始终为 16 MiB。
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.max_audio_bytes > 0 && self.max_audio_bytes <= MAX_VOICE_BYTES,
            "invalid prepared audio byte limit"
        );
        ensure!(
            self.max_response_bytes > 0,
            "invalid prepared audio response limit"
        );
        Ok(())
    }
}

/// 固定版本的内部 IPC 数据；不派生 Debug，避免日志误打原始 base64。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedAudio {
    version: u8,
    silk_base64: String,
    silk_sha256: String,
    silk_size_bytes: usize,
    #[serde(with = "Evidence")]
    evidence: VoiceEvidence,
}

// 仅描述现有证据的 wire 形状，不创建第二套归属或数据库关联算法。
#[derive(Serialize, Deserialize)]
#[serde(remote = "VoiceEvidence", deny_unknown_fields)]
struct Evidence {
    username: String,
    message_source: String,
    message_table: String,
    message_local_id: i64,
    server_id: i64,
    create_time: i64,
    media_source: String,
    media_rowid: i64,
    media_chat_name_id: i64,
    media_local_id: i64,
}

/// 从已验证数据库结果生成完整 JSON；超限时不交付部分响应。
pub fn encode(voice: &DatabaseVoice, limits: Limits) -> Result<Vec<u8>> {
    limits.validate()?;
    validate_size(voice.silk.len(), limits)?;
    validate_silk(&voice.silk)?;
    validate_evidence(&voice.evidence)?;
    ensure!(
        encoded_len(voice.silk.len()) <= limits.max_response_bytes,
        "prepared audio response exceeds limit"
    );
    let prepared = PreparedAudio {
        version: 1,
        silk_base64: STANDARD.encode(&voice.silk),
        silk_sha256: format!("{:x}", Sha256::digest(&voice.silk)),
        silk_size_bytes: voice.silk.len(),
        evidence: voice.evidence.clone(),
    };
    let mut writer = BoundedJson {
        bytes: Vec::new(),
        limit: limits.max_response_bytes,
    };
    serde_json::to_writer(&mut writer, &prepared).map_err(|_| {
        anyhow::anyhow!("prepared audio response exceeds limit or cannot serialize")
    })?;
    Ok(writer.bytes)
}

/// 宿主入口：先限制 JSON，再检查证据、base64 长度、解码长度和 SHA-256。
/// 只有全部校验成功才返回 SILK，调用方此后才能调用 WAV/ASR 管线。
/// 校验和不是签名；需要认证的调用方仍须绑定受信任 IPC 通道及请求身份。
pub fn decode(payload: &[u8], limits: Limits) -> Result<DatabaseVoice> {
    limits.validate()?;
    ensure!(
        payload.len() <= limits.max_response_bytes,
        "prepared audio response exceeds limit"
    );
    let prepared: PreparedAudio = serde_json::from_slice(payload)
        .map_err(|_| anyhow::anyhow!("invalid prepared audio JSON"))?;
    ensure!(prepared.version == 1, "unsupported prepared audio version");
    validate_size(prepared.silk_size_bytes, limits)?;
    validate_evidence(&prepared.evidence)?;
    ensure!(
        prepared.silk_base64.len() == encoded_len(prepared.silk_size_bytes),
        "prepared audio base64 length mismatch"
    );
    ensure!(
        prepared.silk_sha256.len() == 64
            && prepared
                .silk_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid prepared audio SHA256"
    );
    // 在 base64 解码前锁定分配上限，不让声明长度或恶意编码推动无界增长。
    let mut silk = vec![0; prepared.silk_size_bytes];
    let decoded = STANDARD
        .decode_slice(&prepared.silk_base64, &mut silk)
        .map_err(|_| anyhow::anyhow!("invalid prepared audio base64"))?;
    ensure!(
        decoded == prepared.silk_size_bytes,
        "prepared audio size mismatch"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&silk)) == prepared.silk_sha256,
        "prepared audio SHA256 mismatch"
    );
    validate_silk(&silk)?;
    Ok(DatabaseVoice {
        silk,
        evidence: prepared.evidence,
    })
}

fn validate_size(size: usize, limits: Limits) -> Result<()> {
    ensure!(
        size > 0 && size <= limits.max_audio_bytes,
        "prepared audio bytes exceed limit or are empty"
    );
    Ok(())
}

fn encoded_len(size: usize) -> usize {
    // 调用前已验证 16 MiB 硬上限，计算不会溢出。
    size.div_ceil(3) * 4
}

fn validate_silk(silk: &[u8]) -> Result<()> {
    ensure!(
        silk.strip_prefix(&[2])
            .unwrap_or(silk)
            .starts_with(b"#!SILK_V3"),
        "invalid prepared audio SILK header"
    );
    Ok(())
}

fn validate_evidence(e: &VoiceEvidence) -> Result<()> {
    ensure!(
        crate::adapters::wechat::messages::read::layout::valid_voice_source(
            &e.username,
            &e.message_source,
            &e.media_source,
            &e.message_table,
            e.message_local_id,
            e.server_id,
            None,
        ),
        "invalid prepared audio evidence"
    );
    Ok(())
}

struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("prepared audio response limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-audio/prepared_tests.rs"]
mod tests;
