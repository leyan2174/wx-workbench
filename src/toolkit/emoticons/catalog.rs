//! 旧 emoticons.py 的只读映射；解密与 WAL 生命周期由账号 DbCache 管理。

use std::{collections::HashMap, path::Path};

use anyhow::{anyhow, bail, Context, Result};
use regex::{Captures, Regex};
use rusqlite::{Connection, OpenFlags};

use super::types::{Catalog, Emoji, EmojiInfo};
use crate::daemon::cache::DbCache;

fn empty() -> Catalog {
    Catalog {
        items: Vec::new(),
        non_store_count: 0,
        store_added: 0,
    }
}

/// 缺密钥或源文件返回空映射；规范化重名、解密和读取失败返回错误。
/// 仅匹配完整相对路径，传给 DbCache 的仍然是未改写的原始键。
pub async fn load(cache: &DbCache) -> Result<Catalog> {
    let keys: Vec<_> = cache
        .raw_db_keys()
        .into_iter()
        .filter(|key| {
            key.replace('\\', "/")
                .eq_ignore_ascii_case("emoticon/emoticon.db")
        })
        .collect();
    if keys.len() > 1 {
        bail!("emoticon/emoticon.db 存在规范化重名键");
    }
    let Some(key) = keys.first() else {
        return Ok(empty());
    };
    let Some(path) = cache
        .get(key)
        .await
        .map_err(|_| anyhow!("读取账号表情数据库缓存失败"))?
    else {
        return Ok(empty());
    };
    tokio::task::spawn_blocking(move || load_from_path(&path))
        .await
        .map_err(|_| anyhow!("表情数据库读取任务失败"))?
}

/// 只读已解密 SQLite；不创建数据库，不解密，不输出 URL 或密钥材料。
pub fn load_from_path(path: &Path) -> Result<Catalog> {
    // SQLite 错误可携带来自 schema 的任意名称；不向调用者保留原始错误链。
    read_catalog(path).map_err(|_| anyhow!("读取表情数据库失败"))
}

fn read_catalog(path: &Path) -> Result<Catalog> {
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("打开表情数据库失败")?;
    // 三阶段共用读取快照；任何行失败都不能返回部分映射。
    let tx = conn.transaction().context("建立表情读取快照失败")?;
    let mut catalog = empty();
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut templates = HashMap::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT md5, aes_key, cdn_url, encrypt_url, product_id FROM kNonStoreEmoticonTable",
            )
            .context("读取 NonStore schema 失败")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                EmojiInfo {
                    aes_key: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    cdn_url: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    encrypt_url: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    product_id: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    caption: None,
                },
            ))
        })?;
        for row in rows {
            let (md5, info) = row.context("读取 NonStore 行失败")?;
            // 无 MD5 的行仍可提供模板；空 URL 不会清除前一个模板。
            if !info.product_id.is_empty() && !info.cdn_url.is_empty() {
                templates.insert(info.product_id.clone(), info.cdn_url.clone());
            }
            if !md5.is_empty() {
                if let Some(&index) = positions.get(&md5) {
                    catalog.items[index].info = info;
                } else {
                    positions.insert(md5.clone(), catalog.items.len());
                    catalog.items.push(Emoji { md5, info });
                }
            }
        }
    }
    catalog.non_store_count = catalog.items.len();
    {
        let replacement = Regex::new(r"m=[0-9a-f]+")?;
        let mut stmt = tx
            .prepare("SELECT package_id_, md5_ FROM kStoreEmoticonFilesTable")
            .context("读取 Store schema 失败")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?;
        for row in rows {
            let (package_id, md5) = row.context("读取 Store 行失败")?;
            if md5.is_empty() || positions.contains_key(&md5) {
                continue;
            }
            if let Some(template) = templates.get(&package_id).filter(|url| url.contains('&')) {
                // 保留旧正则的部分匹配、非参数边界匹配和全部替换行为。
                let parts = replacement_parts(&md5)?;
                let cdn_url = replacement
                    .replace_all(template, |captures: &Captures<'_>| parts.join(&captures[0]))
                    .into_owned();
                positions.insert(md5.clone(), catalog.items.len());
                catalog.items.push(Emoji {
                    md5,
                    info: EmojiInfo {
                        cdn_url,
                        product_id: package_id,
                        ..EmojiInfo::default()
                    },
                });
                catalog.store_added += 1;
            }
        }
    }
    // 只允许缺少可选表；坏列、坏行、视图故障、锁与损坏必须上报。
    let has_captions: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'kStoreEmoticonCaptionsTable' COLLATE NOCASE AND type IN ('table', 'view'))",
        [], |row| row.get(0),
    ).context("检查 caption schema 失败")?;
    if has_captions {
        let mut stmt = tx
            .prepare(
                "SELECT md5_, caption_ FROM kStoreEmoticonCaptionsTable WHERE language_='default'",
            )
            .context("读取 caption schema 失败")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?;
        for row in rows {
            let (md5, caption) = row.context("读取 caption 行失败")?;
            if let Some(index) = md5.as_ref().and_then(|md5| positions.get(md5)) {
                catalog.items[*index].info.caption = Some(caption);
            }
        }
    }
    tx.commit().context("结束表情读取快照失败")?;
    Ok(catalog)
}

// Python re.sub 的替换串会解释转义；本模式没有捕获组，只有 \g<0> 可引用。
// 各段之间插入完整匹配。即使模板没有匹配，也必须先验证替换串。
fn replacement_parts(md5: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut literal = String::from("m=");
    let mut chars = md5.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            literal.push(ch);
            continue;
        }
        let escape = chars.next().context("无效表情替换串")?;
        match escape {
            'g' => {
                if chars.next() != Some('<') {
                    bail!("无效表情替换串");
                }
                let mut group = String::new();
                loop {
                    match chars.next() {
                        Some('>') => break,
                        Some(c) => group.push(c),
                        None => bail!("无效表情替换串"),
                    }
                }
                if group.is_empty() || !group.chars().all(|c| c == '0') {
                    bail!("无效表情替换串");
                }
                parts.push(std::mem::take(&mut literal));
            }
            '0'..='9' => {
                let tail: String = chars.clone().take(2).collect();
                let octal = |c: char| ('0'..='7').contains(&c);
                if escape != '0' && !(octal(escape) && tail.len() == 2 && tail.chars().all(octal)) {
                    bail!("无效表情替换串");
                }
                let mut digits = String::from(escape);
                for _ in 0..2 {
                    if chars.peek().copied().is_some_and(octal) {
                        digits.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                let value = u32::from_str_radix(&digits, 8)?;
                if value > 255 {
                    bail!("无效表情替换串");
                }
                literal.push(char::from_u32(value).unwrap());
            }
            '\\' => literal.push('\\'),
            'a' => literal.push('\x07'),
            'b' => literal.push('\x08'),
            'f' => literal.push('\x0c'),
            'n' => literal.push('\n'),
            'r' => literal.push('\r'),
            't' => literal.push('\t'),
            'v' => literal.push('\x0b'),
            c if c.is_ascii_alphabetic() => bail!("无效表情替换串"),
            c => {
                literal.push('\\');
                literal.push(c);
            }
        }
    }
    parts.push(literal);
    Ok(parts)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
