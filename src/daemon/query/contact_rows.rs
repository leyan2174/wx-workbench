//! 联系人返回行只读查询；路径与账号绑定由调用者完成，不发现配置或其他副本。
use anyhow::{ensure, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;

const MAX_ROWS: usize = 100_000;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

/// 对齐旧 MCP 的范围、搜索及扫描顺序，保留 CLI 的 contacts/username/display/total。
/// local_type 存在时只排除 3（SQL 同时排除 NULL）；不存在时保留全部行。
pub fn contacts_from_path(path: &Path, query: Option<&str>, limit: usize) -> Result<Value> {
    let query = query.unwrap_or("");
    ensure!(
        query.len() <= MAX_TEXT_BYTES,
        "contact query byte limit exceeded"
    );
    let query = query.to_lowercase();
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let snapshot = conn.unchecked_transaction()?;
    let columns = snapshot
        .prepare("PRAGMA table_info(contact)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // 可选列与 contact_metadata 一样精确匹配，必需列由 SQLite 解析大小写。
    let optional = [
        "alias",
        "description",
        "phone",
        "phone_number",
        "mobile",
        "mobile_phone",
        "telephone",
    ];
    let mut selected = vec![
        "[username]".to_owned(),
        "[nick_name]".to_owned(),
        "[remark]".to_owned(),
    ];
    selected.extend(optional.iter().map(|name| {
        if columns.iter().any(|column| column == name) {
            format!("[{name}]")
        } else {
            "NULL".into()
        }
    }));
    let filter = if columns.iter().any(|column| column == "local_type") {
        " WHERE local_type != 3"
    } else {
        ""
    };
    let mut statement = snapshot.prepare(&format!(
        "SELECT {} FROM contact{filter}",
        selected.join(", ")
    ))?;
    let mut rows = statement.query([])?;
    let mut contacts = Vec::new();
    let (mut scanned, mut total, mut result_bytes) = (0, 0, 0);
    while let Some(row) = rows.next()? {
        scanned += 1;
        ensure!(scanned <= MAX_ROWS, "contact row limit exceeded");
        let fields = (0..selected.len())
            .map(|index| text(row.get_ref(index)?))
            .collect::<Result<Vec<_>>>()?;
        let username = &fields[0];
        ensure!(!username.is_empty(), "contact username is empty");
        let nick = &fields[1];
        let remark = &fields[2];
        if !query.is_empty()
            && ![username, nick, remark]
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
        {
            continue;
        }
        total += 1;
        if contacts.len() >= limit {
            continue;
        }
        let display = if !remark.is_empty() {
            remark
        } else if !nick.is_empty() {
            nick
        } else {
            username
        };
        let phone = fields[5..]
            .iter()
            .find(|value| !value.is_empty())
            .map(String::as_str)
            .unwrap_or("");
        let contact = json!({"username":username,"nick_name":nick,"remark":remark,"alias":fields[3],"description":fields[4],"phone":phone,"display":display});
        result_bytes += serde_json::to_vec(&contact)?.len();
        ensure!(
            result_bytes <= MAX_RESULT_BYTES,
            "contact result byte limit exceeded"
        );
        contacts.push(contact);
    }
    ensure!(scanned > 0, "contact database contains no eligible rows");
    Ok(json!({"contacts":contacts,"total":total}))
}

// 与既有元数据读取的空值回退一致；联系人搜索只接受文本，不将二进制转成名称。
fn text(value: ValueRef<'_>) -> Result<String> {
    Ok(match value {
        ValueRef::Null | ValueRef::Integer(0) | ValueRef::Real(0.0) => String::new(),
        ValueRef::Blob(bytes) if bytes.is_empty() => String::new(),
        ValueRef::Text(bytes) => {
            ensure!(
                bytes.len() <= MAX_TEXT_BYTES,
                "contact text byte limit exceeded"
            );
            std::str::from_utf8(bytes)?.to_owned()
        }
        _ => anyhow::bail!("contact field must be text"),
    })
}
