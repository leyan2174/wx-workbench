use super::output::{print_value, resolve};
use crate::business::favorites::FavoriteKind;
use crate::ipc::Request;
use crate::service::query_client as transport;
use anyhow::{ensure, Context, Result};

fn parse_fav_type(s: &str) -> Result<FavoriteKind> {
    match s {
        "text" => Ok(FavoriteKind::Text),
        "image" => Ok(FavoriteKind::Image),
        "article" => Ok(FavoriteKind::Article),
        "card" => Ok(FavoriteKind::ContactCard),
        "video" => Ok(FavoriteKind::Video),
        _ => anyhow::bail!("不支持的收藏类型"),
    }
}

fn request(limit: usize, fav_type: Option<&str>, query: Option<String>) -> Result<Request> {
    let kind = fav_type.map(parse_fav_type).transpose()?;
    let fav_type = kind
        .map(|kind| {
            crate::service::favorite_filter::legacy_wire_type(kind)
                .context("收藏类型没有兼容请求表示")
        })
        .transpose()?;
    Ok(Request::Favorites {
        limit,
        fav_type,
        query,
    })
}

fn page(data: &serde_json::Value) -> Result<(&serde_json::Value, bool)> {
    let items = data
        .get("items")
        .filter(|value| value.is_array())
        .context("收藏响应缺少有效 items")?;
    let has_more = data
        .get("has_more")
        .and_then(serde_json::Value::as_bool)
        .context("收藏响应缺少有效 has_more")?;
    let count = data
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .context("收藏响应缺少有效 count")?;
    ensure!(
        count == items.as_array().unwrap().len() as u64,
        "收藏响应 count 与 items 不一致"
    );
    ensure!(
        items
            .as_array()
            .unwrap()
            .iter()
            .all(serde_json::Value::is_object),
        "收藏响应包含无效条目"
    );
    Ok((items, has_more))
}

fn continuation_notice(has_more: bool) -> Option<&'static str> {
    has_more.then_some("[wx] 收藏结果尚未结束；可增大 --limit 或缩小筛选范围。")
}

pub fn cmd_favorites(
    limit: usize,
    fav_type: Option<String>,
    query: Option<String>,
    json: bool,
) -> Result<()> {
    let resp = transport::send(request(limit, fav_type.as_deref(), query)?)?;
    let (items, has_more) = page(&resp.data)?;
    print_value(items, &resolve(json))?;
    if let Some(notice) = continuation_notice(has_more) {
        eprintln!("{notice}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
