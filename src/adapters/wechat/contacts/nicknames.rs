//! Existing WeChat group-card parser; compatibility heuristics remain adapter-private.
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

pub(crate) fn load_group_nickname_map_from_conn(
    conn: &Connection,
    chat_username: &str,
    targets: Option<&HashSet<String>>,
) -> HashMap<String, String> {
    if !chat_username.contains("@chatroom") {
        return HashMap::new();
    }
    let ext = load_group_ext_buffer(conn, chat_username);

    let owned_targets = if targets.is_none() {
        load_group_member_username_set(conn, chat_username)
    } else {
        None
    };
    let targets = targets.or(owned_targets.as_ref());

    ext.as_deref()
        .map(|buf| parse_group_nickname_map(buf, targets))
        .unwrap_or_default()
}

fn load_group_ext_buffer(conn: &Connection, chat_username: &str) -> Option<Vec<u8>> {
    [
        "SELECT ext_buffer FROM chat_room WHERE username = ? LIMIT 1",
        "SELECT ext_buffer FROM chat_room WHERE chat_room_name = ? LIMIT 1",
        "SELECT ext_buffer FROM chat_room WHERE name = ? LIMIT 1",
    ]
    .iter()
    .find_map(|sql| {
        conn.query_row(sql, [chat_username], |row| row.get::<_, Option<Vec<u8>>>(0))
            .ok()
            .flatten()
    })
}

fn load_group_member_username_set(
    conn: &Connection,
    chat_username: &str,
) -> Option<HashSet<String>> {
    let room_id: i64 = [
        "SELECT id FROM chat_room WHERE username = ?",
        "SELECT id FROM chat_room WHERE chat_room_name = ?",
        "SELECT id FROM chat_room WHERE name = ?",
    ]
    .iter()
    .find_map(|sql| {
        conn.query_row(sql, [chat_username], |row| row.get::<_, i64>(0))
            .ok()
    })
    .unwrap_or(0);

    if room_id == 0 {
        return None;
    }

    let mut stmt = conn
        .prepare(
            "SELECT c.username
         FROM chatroom_member cm
         LEFT JOIN contact c ON c.id = cm.member_id
         WHERE cm.room_id = ?",
        )
        .ok()?;
    let usernames: HashSet<String> = stmt
        .query_map([room_id], |row| row.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .filter(|uid| !uid.is_empty())
        .collect();

    if usernames.is_empty() {
        None
    } else {
        Some(usernames)
    }
}

pub(crate) fn decode_proto_varint(raw: &[u8], offset: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut pos = offset;
    while pos < raw.len() {
        let byte = raw[pos];
        pos += 1;
        // u64 的第十字节只能携带最低一位；其余位不能被移位静默丢弃。
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, pos));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

fn proto_len_fields(raw: &[u8]) -> Vec<(u64, &[u8])> {
    let mut fields = Vec::new();
    let mut idx = 0usize;
    while idx < raw.len() {
        let Some((tag, next)) = decode_proto_varint(raw, idx) else {
            break;
        };
        if next <= idx {
            break;
        }
        idx = next;
        let field_no = tag >> 3;
        let wire_type = tag & 0x07;
        match wire_type {
            0 => {
                let Some((_, next)) = decode_proto_varint(raw, idx) else {
                    break;
                };
                if next <= idx {
                    break;
                }
                idx = next;
            }
            1 => {
                let Some(next) = idx.checked_add(8) else {
                    break;
                };
                if next > raw.len() {
                    break;
                }
                idx = next;
            }
            2 => {
                let Some((size, next)) = decode_proto_varint(raw, idx) else {
                    break;
                };
                if next <= idx {
                    break;
                }
                idx = next;
                let Ok(size) = usize::try_from(size) else {
                    break;
                };
                let Some(end) = idx.checked_add(size) else {
                    break;
                };
                if end > raw.len() {
                    break;
                }
                fields.push((field_no, &raw[idx..end]));
                idx = end;
            }
            5 => {
                let Some(next) = idx.checked_add(4) else {
                    break;
                };
                if next > raw.len() {
                    break;
                }
                idx = next;
            }
            _ => break,
        }
    }
    fields
}

fn proto_string_fields(raw: &[u8]) -> Vec<(u64, String)> {
    proto_len_fields(raw)
        .into_iter()
        .filter_map(|(field_no, value)| {
            if value.is_empty() || value.len() > 256 {
                return None;
            }
            let text = std::str::from_utf8(value).ok()?.trim().to_string();
            if text.is_empty() || text.chars().any(char::is_control) {
                return None;
            }
            Some((field_no, text))
        })
        .collect()
}

fn is_strong_username_hint(value: &str) -> bool {
    value.starts_with("wxid_")
        || value.ends_with("@chatroom")
        || value.starts_with("gh_")
        || value.contains('@')
}

fn looks_like_username(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if is_strong_username_hint(value) {
        return true;
    }
    if value.len() < 6 || value.len() > 32 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn pick_member_username(
    strings: &[(u64, String)],
    targets: Option<&HashSet<String>>,
) -> Option<String> {
    if let Some(targets) = targets {
        return strings
            .iter()
            .find(|(_, value)| targets.contains(value))
            .map(|(_, value)| value.clone());
    }

    for field_no in [1u64, 4u64] {
        if let Some((_, value)) = strings
            .iter()
            .find(|(f, value)| *f == field_no && looks_like_username(value))
        {
            return Some(value.clone());
        }
    }

    strings
        .iter()
        .find(|(_, value)| is_strong_username_hint(value))
        .or_else(|| strings.iter().find(|(_, value)| looks_like_username(value)))
        .map(|(_, value)| value.clone())
}

fn pick_group_nickname(strings: &[(u64, String)], username: &str) -> Option<String> {
    let mut best_score = i64::MIN;
    let mut best = String::new();

    for (idx, (field_no, value)) in strings.iter().enumerate() {
        // In current WeChat 4.x ext_buffer member chunks, field 2 is the group
        // card/nickname. Field 4 is often another username-like value such as an
        // inviter/owner and must not be promoted to a nickname.
        if *field_no != 2 {
            continue;
        }
        let value = value.trim();
        if value.is_empty()
            || value == username
            || is_strong_username_hint(value)
            || value.contains('\n')
            || value.contains('\r')
            || value.len() > 64
        {
            continue;
        }

        let mut score = 0i64;
        if !looks_like_username(value) {
            score += 20;
        }
        score += (32usize.saturating_sub(value.len())) as i64;
        score = score * 1000 - idx as i64;

        if score > best_score {
            best_score = score;
            best = value.to_string();
        }
    }

    if best.is_empty() {
        None
    } else {
        Some(best)
    }
}

pub(crate) fn parse_group_nickname_map(
    ext_buffer: &[u8],
    targets: Option<&HashSet<String>>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if ext_buffer.is_empty() {
        return out;
    }

    for (_, chunk) in proto_len_fields(ext_buffer) {
        let strings = proto_string_fields(chunk);
        if strings.is_empty() {
            continue;
        }
        let Some(username) = pick_member_username(&strings, targets) else {
            continue;
        };
        if out.contains_key(&username) {
            continue;
        }
        if let Some(nickname) = pick_group_nickname(&strings, &username) {
            out.insert(username, nickname);
        }
    }

    out
}
