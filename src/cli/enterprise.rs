//! 企业微信离线主库入口；密钥只从明确指定的文件读取，使用后清零。
use crate::toolkit::enterprise::queries::{ExportFormat, MessageFilter, OfflineStore};
use anyhow::{ensure, Context, Result};
use std::{fs, io::Read, path::PathBuf};
use zeroize::Zeroizing;

#[derive(clap::ValueEnum, Clone, Copy)]
pub enum Format {
    Json,
    Csv,
    Html,
}

#[derive(clap::Subcommand)]
pub enum QueryCommand {
    /// 列出联系人
    Contacts,
    /// 列出有消息的会话
    Conversations,
    /// 按条件查询消息；时间使用数据库原始单位，含起点不含终点
    Messages {
        #[arg(long, value_delimiter = ',')]
        conversations: Vec<String>,
        #[arg(long)]
        start: Option<i64>,
        #[arg(long)]
        end: Option<i64>,
        #[arg(long)]
        sender: Option<i64>,
        #[arg(long)]
        content_type: Option<i64>,
        #[arg(long)]
        contains: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 导出一个完整会话；输出父目录须存在，禁止覆盖
    Export {
        conversation: String,
        output: PathBuf,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
    },
}

pub fn cmd_query(snapshot: PathBuf, self_id: Option<i64>, command: QueryCommand) -> Result<()> {
    let store = OfflineStore::open(&snapshot, self_id)?;
    let result = match command {
        QueryCommand::Contacts => serde_json::to_value(store.contacts()?)?,
        QueryCommand::Conversations => serde_json::to_value(store.conversations()?)?,
        QueryCommand::Messages {
            conversations,
            start,
            end,
            sender,
            content_type,
            contains,
            offset,
            limit,
        } => serde_json::to_value(store.messages(&MessageFilter {
            conversation_ids: conversations,
            start_time: start,
            end_time: end,
            sender_id: sender,
            content_type,
            contains,
            offset,
            limit,
        })?)?,
        QueryCommand::Export {
            conversation,
            output,
            format,
        } => {
            let format = match format {
                Format::Json => ExportFormat::Json,
                Format::Csv => ExportFormat::Csv,
                Format::Html => ExportFormat::Html,
            };
            serde_json::to_value(store.export_conversation(&conversation, &output, format)?)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn cmd_decrypt(input: PathBuf, output: PathBuf, key_file: PathBuf) -> Result<()> {
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(&key_file)
        .context("无法打开密钥文件")?
        .take(129)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 128, "密钥文件只能包含 32 位十六进制原始密钥");
    let key_text = std::str::from_utf8(&bytes).context("密钥文件必须为 UTF-8 文本")?;
    let key = Zeroizing::new(crate::toolkit::enterprise::parse_key_hex(
        key_text.trim_start_matches('\u{feff}'),
    )?);
    let output = std::path::absolute(output)?;
    fs::create_dir_all(output.parent().context("输出路径缺少父目录")?)?;
    let report = crate::toolkit::enterprise::decrypt_database(&input, &output, &key)?;
    use crate::toolkit::enterprise::DatabaseFormat;
    let format = match report.format {
        DatabaseFormat::PlainSqlite => "sqlite",
        DatabaseFormat::WxSqlite3Aes128Header => "wxsqlite3_aes128_header",
        DatabaseFormat::WxSqlite3Aes128Legacy => "wxsqlite3_aes128_legacy",
    };
    println!(
        "{}",
        serde_json::json!({"engine":"rust","output":output,"pages":report.pages,"bytes":report.bytes,"format":format})
    );
    Ok(())
}
