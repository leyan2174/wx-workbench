//! 导出单个会话；先写临时文件，成功后原子不覆盖发布。
use super::{Conversation, Message, MessageFilter, OfflineStore};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
    Html,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub conversation_id: String,
    pub message_count: usize,
    pub output: PathBuf,
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn safe_cell(value: &str) -> String {
    // CSV 中的文本字段禁止被 Excel 解释成公式；JSON 保留原始内容。
    if value.trim_start().starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r', '\n'])
    {
        format!("'{value}")
    } else {
        value.into()
    }
}

fn write_csv(file: &mut File, messages: &[Message]) -> Result<()> {
    file.write_all(b"\xef\xbb\xbf")?;
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_writer(file);
    writer.write_record([
        "时间",
        "会话",
        "会话ID",
        "发送者",
        "发送者ID",
        "消息类型",
        "内容",
        "message_id",
        "server_id",
        "sequence",
        "flag",
    ])?;
    for m in messages {
        writer.write_record([
            m.time.clone(),
            safe_cell(&m.conversation),
            safe_cell(&m.conversation_id),
            safe_cell(&m.sender),
            m.sender_id.to_string(),
            safe_cell(&m.type_name),
            safe_cell(&m.display_content),
            m.message_id.to_string(),
            m.server_id.to_string(),
            m.sequence.to_string(),
            m.flag.to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_html(file: &mut File, conv: &Conversation, messages: &[Message]) -> Result<()> {
    write!(file, "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\"><title>{}</title><style>body{{font:14px sans-serif;margin:0;color:#222;background:#f5f5f5}}header,main{{max-width:880px;margin:auto;padding:16px}}article{{margin:12px 0;border-left:3px solid #777;padding:8px 12px;background:white;overflow-wrap:anywhere}}article.sent{{border-color:#22844a}}p{{white-space:pre-wrap}}small{{color:#555}}h1{{font-size:22px;overflow-wrap:anywhere}}</style></head><body><header><h1>{}</h1><p>{} · {} 条消息 · {}</p></header><main>", escape(&conv.display_name),escape(&conv.display_name),escape(&conv.kind),messages.len(),escape(&conv.conversation_id))?;
    for m in messages {
        write!(
            file,
            "<article class=\"{}\"><strong>{}</strong><p>{}</p><small>{} · {}</small></article>",
            if m.is_sent { "sent" } else { "received" },
            escape(&m.sender),
            escape(&m.display_content),
            escape(&m.type_name),
            escape(&m.time)
        )?;
    }
    file.write_all(b"</main></body></html>")?;
    Ok(())
}

impl OfflineStore {
    /// 每次导出一个完整会话，忽略分页；输出父目录须已存在且位于源快照目录之外。
    /// 目标不可已存在；目标文件系统须支持硬链接。没有账号扫描、环境变量或配置副作用。
    pub fn export_conversation(
        &self,
        conversation_id: &str,
        output: &Path,
        format: ExportFormat,
    ) -> Result<ExportReport> {
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).context("Export parent directory must exist")?;
        ensure!(
            !parent.starts_with(&self.directory),
            "Export must be outside the source snapshot directory"
        );
        let output = parent.join(output.file_name().context("Export requires a filename")?);
        match fs::symlink_metadata(&output) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => anyhow::bail!("Export already exists; refusing to overwrite"),
        }
        let conv = self
            .conversations()?
            .into_iter()
            .find(|c| c.conversation_id == conversation_id)
            .context("Conversation does not exist or has no messages")?;
        let messages = self.messages(&MessageFilter {
            conversation_ids: vec![conversation_id.into()],
            ..Default::default()
        })?;
        let (temp, mut file) = (0..100)
            .find_map(|_| {
                let path = parent.join(format!(
                    ".wx-enterprise-export-{}-{}.tmp",
                    std::process::id(),
                    SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
                match OpenOptions::new().create_new(true).write(true).open(&path) {
                    Ok(file) => Some(Ok((Temporary(path), file))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .context("Cannot reserve temporary export")??;
        match format {
            ExportFormat::Csv => write_csv(&mut file, &messages)?,
            ExportFormat::Html => write_html(&mut file, &conv, &messages)?,
            ExportFormat::Json => {
                #[derive(Serialize)]
                struct Document<'a> {
                    conversation: &'a Conversation,
                    message_count: usize,
                    messages: &'a [Message],
                }
                serde_json::to_writer_pretty(
                    &mut file,
                    &Document {
                        conversation: &conv,
                        message_count: messages.len(),
                        messages: &messages,
                    },
                )?;
                file.write_all(b"\n")?;
            }
        }
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temp.0, &output)
            .context("Atomic export publish failed (hard-link support required)")?;
        Ok(ExportReport {
            conversation_id: conversation_id.into(),
            message_count: messages.len(),
            output,
        })
    }
}
