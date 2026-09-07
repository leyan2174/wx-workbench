//! 消费人工编辑的计划 CSV；只以 username 选择，不用显示名、序号或统计列绑定身份。
use anyhow::{ensure, Context, Result};
use std::{collections::HashSet, fs::File, io::Read, path::Path};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum Mode {
    #[default]
    Blacklist,
    Whitelist,
}

pub struct Plan {
    selected: Vec<String>,
}

impl Plan {
    pub fn load(path: &Path, mode: Mode) -> Result<Self> {
        let file = File::open(path).context("无法打开计划 CSV")?;
        ensure!(file.metadata()?.is_file(), "计划 CSV 必须是普通文件");
        Self::read(file, mode)
    }

    pub fn read(reader: impl Read, mode: Mode) -> Result<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_BYTES, "计划 CSV 超过 16 MiB");
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        validate_quotes(bytes)?;
        let mut csv = csv::ReaderBuilder::new().flexible(false).from_reader(bytes);
        let headers = csv.headers().context("计划 CSV 表头无效")?.clone();
        let mut unique = HashSet::new();
        for header in &headers {
            ensure!(
                !header.is_empty() && unique.insert(header),
                "计划 CSV 表头为空或重复"
            );
        }
        let username_column = headers
            .iter()
            .position(|name| name == "username")
            .context("计划 CSV 缺少 username 列")?;
        let export_column = headers.iter().position(|name| name == "export");
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for (index, row) in csv.records().enumerate() {
            ensure!(index < MAX_ROWS, "计划 CSV 超过 100000 行");
            let row = row.with_context(|| format!("计划 CSV 第 {} 条记录无效", index + 2))?;
            let username = row[username_column].trim();
            ensure!(
                !username.is_empty()
                    && username.len() <= 4096
                    && !username.chars().any(char::is_control),
                "计划 CSV 第 {} 条记录 username 无效",
                index + 2
            );
            ensure!(
                seen.insert(username.to_owned()),
                "计划 CSV username 重复（第 {} 条记录）",
                index + 2
            );
            let flag = export_column
                .and_then(|column| row.get(column))
                .unwrap_or("")
                .trim();
            let include = match mode {
                Mode::Blacklist => flag != "0",
                Mode::Whitelist => flag == "1",
            };
            if include {
                selected.push(username.to_owned());
            }
        }
        Ok(Self { selected })
    }

    /// valid 必须是已应用 --users / 环境变量过滤的精确账号会话集合。
    /// 与旧脚本一致：跳过的未知账号不报错，选中的未知账号整批拒绝。
    pub fn select<'a>(&self, valid: impl IntoIterator<Item = &'a str>) -> Result<&[String]> {
        let mut usernames = HashSet::new();
        for username in valid {
            ensure!(
                usernames.insert(username),
                "会话清单 username 重复，无法唯一绑定计划"
            );
        }
        ensure!(
            self.selected
                .iter()
                .all(|username| usernames.contains(username.as_str())),
            "计划选中的 username 不在当前会话或 --users 选择集内"
        );
        Ok(&self.selected)
    }
}

// csv crate 宽容未闭合/错位引号；此处只拒绝损坏的引号边界，不分列或解码字段。
// 字段拆分、转义、UTF-8 和列数验证仍完全交给 csv crate。
fn validate_quotes(bytes: &[u8]) -> Result<()> {
    let mut state = 0;
    for &byte in bytes {
        state = match (state, byte) {
            (0, b'"') | (3, b'"') => 2,
            (2, b'"') => 3,
            (2, _) => 2,
            (_, b',' | b'\r' | b'\n') => 0,
            (1, b'"') | (3, _) => anyhow::bail!("计划 CSV 引号边界无效"),
            _ => 1,
        };
    }
    ensure!(state != 2, "计划 CSV 引号未闭合");
    Ok(())
}
