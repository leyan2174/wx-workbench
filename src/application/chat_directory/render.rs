//! 目录导出的展示适配；XML 使用现有安全解析器，不执行原文脚本或远程媒体。
use super::{Format, Row};
use crate::message::export::Target;
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_EMBED_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EMBED_PAGE_BYTES: usize = 64 * 1024 * 1024;

// 只读取本次媒体管线生成的暂存副本，不从消息路径、输出目录或 URL 寻找文件。
fn embedded_image(
    relative: &str,
    staging: &Path,
    files: &BTreeMap<PathBuf, PathBuf>,
    remaining: &mut usize,
) -> Result<String> {
    super::safe_relative(relative)?;
    let path = files
        .get(Path::new(relative))
        .context("图片不在暂存清单中")?;
    ensure!(path.parent() == Some(staging), "图片不在本次暂存目录内");
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("暂存文件名无效")?;
    ensure!(
        name.strip_prefix("stage-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        "暂存文件名无效"
    );
    let guard = crate::attachment::local_files::HostOutputGuard::new(staging)?;
    guard.verify_replaceable_file(path)?;
    use std::os::windows::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    let size = file.metadata()?.len();
    ensure!(
        size > 0 && size <= MAX_EMBED_IMAGE_BYTES,
        "图片超过 HTML 嵌入大小限制"
    );
    let encoded_size = (size as usize).div_ceil(3) * 4 + 32;
    ensure!(encoded_size <= *remaining, "本页 HTML 图片嵌入预算耗尽");
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_EMBED_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == size, "读取期间图片发生变化");
    ensure!(
        same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)?,
        "图片文件身份变化"
    );
    guard.verify_replaceable_file(path)?;
    let mime = match crate::attachment::decoder::detect_image_format(&bytes) {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => anyhow::bail!("暂存内容不是受支持的已解码图片"),
    };
    let uri = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
    // HTML 同时在预览和打开原图链接中使用 URI，两份都计入页面预算。
    let cost = uri.len().checked_mul(2).context("HTML 图片长度溢出")?;
    ensure!(cost <= *remaining, "本页 HTML 图片嵌入预算耗尽");
    *remaining -= cost;
    Ok(uri)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn url(path: &str) -> String {
    let mut encoded = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}

fn csv_cell(value: &str) -> String {
    // CSV 是可执行公式载体；完整原文仍在 JSON 中，展示表格将危险前缀强制设为文本。
    if value
        .trim_start_matches(char::is_whitespace)
        .starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r'])
    {
        format!("'{value}")
    } else {
        value.into()
    }
}
pub(super) fn info(target: &Target, doc: &Value) -> String {
    let field = |key: &str| {
        doc.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace(['\r', '\n'], " ")
    };
    format!(
        "username:  {}\nalias:     {}\nnick_name: {}\nremark:    {}\nis_group:  {}\n",
        target.username.replace(['\r', '\n'], " "),
        field("contact_alias"),
        field("contact_nick_name"),
        field("contact_remark"),
        if target.is_group { "True" } else { "False" }
    )
}
pub(super) fn document(
    format: Format,
    target: &Target,
    rows: &[&Row],
    staging: &Path,
    files: &BTreeMap<PathBuf, PathBuf>,
) -> Result<Vec<u8>> {
    match format {
        Format::Json => Ok(serde_json::to_vec_pretty(
            &json!({"chat_username":target.username,"display_name":target.chat,
            "is_group":target.is_group,"message_count":rows.len(),"messages":rows}),
        )?),
        Format::Csv => {
            let mut bytes = vec![0xef, 0xbb, 0xbf];
            let mut writer = csv::WriterBuilder::new()
                .terminator(csv::Terminator::CRLF)
                .from_writer(&mut bytes);
            writer.write_record([
                "时间",
                "发送者",
                "消息类型",
                "内容",
                "图片路径",
                "server_id",
            ])?;
            for row in rows {
                let paths = row
                    .media
                    .iter()
                    .filter_map(|m| m.path.as_deref())
                    .collect::<Vec<_>>()
                    .join("; ");
                let issues = row
                    .media
                    .iter()
                    .filter(|m| m.path.is_none())
                    .map(|m| format!("[媒体 {}: {}]", m.status, m.detail))
                    .collect::<Vec<_>>()
                    .join(" ");
                let body = if issues.is_empty() {
                    row.display_content.clone()
                } else {
                    format!("{}\n{issues}", row.display_content)
                };
                let id = if row.server_id.is_null() {
                    String::new()
                } else {
                    row.server_id
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| row.server_id.to_string())
                };
                writer.write_record(
                    [
                        row.time_str.as_str(),
                        row.sender.as_str(),
                        row.type_name.as_str(),
                        body.as_str(),
                        paths.as_str(),
                        id.as_str(),
                    ]
                    .map(csv_cell),
                )?;
            }
            writer.flush()?;
            drop(writer);
            Ok(bytes)
        }
        Format::Html => Ok(html(target, rows, staging, files).into_bytes()),
    }
}
fn html(
    target: &Target,
    rows: &[&Row],
    staging: &Path,
    files: &BTreeMap<PathBuf, PathBuf>,
) -> String {
    let title = escape(&target.chat);
    let mut out = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><title>{title}</title><style>
*{{box-sizing:border-box}}body{{margin:0;background:#ededed;color:#222;font:14px/1.5 Arial,"Microsoft YaHei",sans-serif;letter-spacing:0}}header{{padding:14px 20px;background:#307947;color:white;overflow-wrap:anywhere}}main{{max-width:900px;margin:auto;padding:12px}}article{{display:flex;margin:12px 0}}article.sent{{justify-content:flex-end}}article.system{{justify-content:center;color:#666}}section{{max-width:85%;min-width:0}}.bubble{{padding:10px;background:white;border-radius:6px;white-space:pre-wrap;overflow-wrap:anywhere}}.sent .bubble{{background:#b8ed9a}}.system .bubble{{background:transparent}}small,time{{color:#626262;font-size:12px}}img,video{{max-width:100%;max-height:600px;display:block}}audio{{width:100%;max-width:360px}}a{{color:#155da6;overflow-wrap:anywhere}}.missing{{color:#984126;margin-top:6px}}h2{{text-align:center;font-size:12px;font-weight:400;color:#666}}details{{margin-top:8px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}@media(max-width:500px){{main{{padding:8px}}section{{max-width:95%}}}}
</style></head><body><header>{title}</header><main>"#
    );
    let mut day = String::new();
    let mut embed_remaining = MAX_EMBED_PAGE_BYTES;
    for row in rows {
        let current = row.time_str.get(..10).unwrap_or("时间未知");
        if current != day {
            out.push_str(&format!("<h2>{}</h2>", escape(current)));
            day = current.into();
        }
        let side = if row.is_system {
            "system"
        } else if row.is_received {
            "received"
        } else {
            "sent"
        };
        out.push_str(&format!("<article class=\"{side}\"><section><small>{}</small> <time>{}</time><div class=\"bubble\">{}",escape(&row.sender),escape(&row.time_str),escape(&row.display_content)));
        for media in &row.media {
            if let Some(path) = &media.path {
                if super::safe_relative(path).is_err() {
                    out.push_str("<div class=\"missing\">[媒体路径不安全，未引用]</div>");
                    continue;
                }
                let href = url(path);
                match media.kind.as_str() {
                    "image" | "sticker" => {
                        let embedded = if media.status == "available" {
                            embedded_image(path, staging, files, &mut embed_remaining)
                        } else {
                            Err(anyhow::anyhow!("图片未通过媒体准备校验"))
                        };
                        let source = embedded.as_deref().unwrap_or(&href);
                        out.push_str(&format!("<a href=\"{source}\"><img loading=\"lazy\" src=\"{source}\" alt=\"{}\"></a>", escape(&media.detail)));
                        if embedded.is_err() {
                            out.push_str("<div class=\"missing\">[图片未嵌入：文件不可用、格式不支持或超过嵌入限额；保留目录引用]</div>");
                        }
                    }
                    "video" => out.push_str(&format!(
                        "<video controls preload=\"metadata\" src=\"{href}\"></video>"
                    )),
                    _ => {}
                }
                out.push_str(&format!(
                    "<a download href=\"{href}\">{}</a>",
                    escape(if media.detail.is_empty() {
                        path
                    } else {
                        &media.detail
                    })
                ));
                if let Some(binding) = &media.binding {
                    out.push_str(&format!("<small> {}</small>", escape(binding)));
                }
            } else {
                out.push_str(&format!(
                    "<div class=\"missing\">[媒体 {}: {}]</div>",
                    escape(&media.status),
                    escape(&media.detail)
                ));
            }
        }
        if row.structured_details {
            out.push_str(&format!(
                "<details><summary>消息详情</summary><pre>{}</pre></details>",
                escape(&serde_json::to_string_pretty(&row.native).unwrap_or_default())
            ));
        }
        out.push_str("</div></section></article>");
    }
    out.push_str("</main></body></html>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=").unwrap()
    }

    fn row(path: &str) -> Row {
        Row {
            local_id: 1,
            server_id: json!(2),
            local_type: 3,
            type_name: "image".into(),
            sort_seq: json!(1),
            sender_username: "synthetic".into(),
            sender: "<sender>".into(),
            create_time: Some(0),
            time_str: "1970-01-01 00:00:00".into(),
            status: json!(0),
            content: json!("synthetic"),
            display_content: "[image]".into(),
            is_system: false,
            structured_details: false,
            is_received: true,
            source: "message/message_0.db".into(),
            media: vec![super::super::Media {
                kind: "image".into(),
                status: "available".into(),
                path: Some(path.into()),
                detail: "<image>".into(),
                binding: Some("synthetic".into()),
                evidence: Value::Null,
            }],
            native: json!({}),
        }
    }

    fn target() -> Target {
        Target {
            username: "synthetic".into(),
            chat: "<synthetic>".into(),
            is_group: false,
        }
    }

    #[test]
    fn raw_voice_download_manifest_and_unknown_metadata() {
        let root = tempfile::tempdir().unwrap();
        let mut row = row("voice/synthetic.silk");
        row.local_type = 34;
        row.sender_username.clear();
        row.create_time = None;
        row.server_id = Value::Null;
        row.content = Value::Null;
        row.media[0].kind = "voice".into();
        row.media[0].binding = None;
        let html = String::from_utf8(
            document(
                Format::Html,
                &target(),
                &[&row],
                root.path(),
                &BTreeMap::new(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(html.contains("voice/synthetic.silk"));
        assert!(!html.contains("<audio"));
        let manifest = super::super::voice_manifest("account", &target(), &[row.clone()]);
        let item = serde_json::to_value(&manifest[0]).unwrap();
        for key in ["message_id", "sender", "timestamp", "duration_ms"] {
            assert!(item[key].is_null(), "{key}");
        }
        assert_eq!(item["relative_path"], "voice/synthetic.silk");
        row.server_id = json!(998);
        row.content = json!("<msg><voicemsg voicelength='1250'/></msg>");
        row.media[0].status = "missing".into();
        row.media[0].path = None;
        let manifest = super::super::voice_manifest("account", &target(), &[row]);
        assert_eq!(manifest[0].duration_ms, Some(1250));
        assert!(manifest[0].message_id.is_some());
        assert_eq!(manifest[0].status, "missing");
        assert_eq!(manifest[0].relative_path, None);
        assert_eq!(manifest[0].encoding, None);
    }

    #[test]
    fn html_embeds_actual_png_bytes_by_default_and_keeps_directory_download() {
        let root = tempfile::tempdir().unwrap();
        let bytes = png();
        let mut files = BTreeMap::new();
        // 扩展名故意不同，MIME 必须来自已解码内容。
        super::super::stage(
            root.path(),
            &mut files,
            PathBuf::from("image/picture.jpg"),
            &bytes,
        )
        .unwrap();
        let row = row("image/picture.jpg");
        let html = String::from_utf8(
            document(Format::Html, &target(), &[&row], root.path(), &files).unwrap(),
        )
        .unwrap();
        let uri = format!("data:image/png;base64,{}", STANDARD.encode(&bytes));
        assert!(html.contains(&format!("src=\"{uri}\"")));
        assert!(html.contains(&format!("href=\"{uri}\"")));
        let encoded = html
            .split("src=\"data:image/png;base64,")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(STANDARD.decode(encoded).unwrap(), bytes);
        assert!(html.contains("<a download href=\"image/picture.jpg\">"));
        assert!(html.contains("&lt;sender&gt;"));
        assert!(!html.contains("图片未嵌入"));
        assert_eq!(
            std::fs::read(&files[Path::new("image/picture.jpg")]).unwrap(),
            bytes
        );
    }

    #[test]
    fn embedding_rejects_external_traversal_unlisted_and_outside_staging_paths() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("stage-0");
        std::fs::write(&path, png()).unwrap();
        let files = BTreeMap::from([(PathBuf::from("image/picture.png"), path.clone())]);
        for relative in [
            "https://untrusted.invalid/image.png",
            "../stage-0",
            "image/../../stage-0",
            "image/unlisted.png",
            "image/picture.png",
        ] {
            let mut budget = MAX_EMBED_PAGE_BYTES;
            assert!(embedded_image(relative, root.path(), &files, &mut budget).is_err());
            assert_eq!(budget, MAX_EMBED_PAGE_BYTES);
        }
        for unsafe_path in ["https://untrusted.invalid/image.png", "../stage-0"] {
            let row = row(unsafe_path);
            let html = html(&target(), &[&row], root.path(), &files);
            assert!(html.contains("媒体路径不安全"));
            assert!(!html.contains("<img "));
            assert!(!html.contains("data:image/"));
        }
        assert_eq!(std::fs::read(path).unwrap(), png());
    }

    #[test]
    fn embedding_failure_is_visible_and_preserves_relative_fallback_and_budget() {
        let root = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        super::super::stage(
            root.path(),
            &mut files,
            PathBuf::from("image/not-image.png"),
            b"<svg onload='unsafe' />",
        )
        .unwrap();
        for path in ["image/not-image.png", "image/missing.png"] {
            let row = row(path);
            let html = html(&target(), &[&row], root.path(), &files);
            assert!(html.contains("图片未嵌入"));
            assert!(html.contains(&format!("src=\"{path}\"")));
            assert!(!html.contains("data:image/"));
        }
        super::super::stage(
            root.path(),
            &mut files,
            PathBuf::from("image/valid.png"),
            &png(),
        )
        .unwrap();
        let mut budget = 1;
        assert!(embedded_image("image/valid.png", root.path(), &files, &mut budget).is_err());
        assert_eq!(budget, 1);
        let mut budget = MAX_EMBED_PAGE_BYTES;
        let uri = embedded_image("image/valid.png", root.path(), &files, &mut budget).unwrap();
        assert_eq!(budget, MAX_EMBED_PAGE_BYTES - uri.len() * 2);
        // 同卷硬链接也不能借暂存文件名绕过普通文件校验。
        std::fs::hard_link(
            &files[Path::new("image/valid.png")],
            root.path().join("alias"),
        )
        .unwrap();
        assert!(embedded_image("image/valid.png", root.path(), &files, &mut budget).is_err());
    }

    #[test]
    fn csv_and_json_do_not_read_or_embed_staged_media() {
        let root = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        super::super::stage(
            root.path(),
            &mut files,
            PathBuf::from("image/picture.png"),
            &png(),
        )
        .unwrap();
        let row = row("image/picture.png");
        for format in [Format::Csv, Format::Json] {
            let before = document(format, &target(), &[&row], root.path(), &files).unwrap();
            let without_files = document(
                format,
                &target(),
                &[&row],
                &root.path().join("absent"),
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(before, without_files);
            assert!(!String::from_utf8_lossy(&before).contains("data:image/"));
            assert!(String::from_utf8_lossy(&before).contains("image/picture.png"));
        }
    }

    #[test]
    fn escapes_untrusted_markup_and_relative_urls() {
        assert_eq!(
            escape("<img onerror='x'>&"),
            "&lt;img onerror=&#39;x&#39;&gt;&amp;"
        );
        assert_eq!(url("file/a #中.pdf"), "file/a%20%23%E4%B8%AD.pdf");
    }
}
