//! 阅读摘要不包含名片认证字段；原始消息及详细解码仍由存储和工具层保留。

use super::xml::{collapse, parse};

/// 播放时长按旧导出原样保留，不将未知格式误当作数字重写。
pub fn video(content: &str) -> String {
    let length = parse(content).and_then(|doc| {
        let node = doc
            .root_element()
            .descendants()
            .skip(1)
            .find(|node| node.has_tag_name("videomsg"))?;
        node.attribute("playlength")
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    length
        .map(|value| format!("[视频] {value}秒"))
        .unwrap_or_else(|| "[视频]".into())
}

/// 保留客户端提供的通话状态，不根据文字猜测语音或视频类型。
pub fn voip(content: &str) -> Option<String> {
    if !content.contains("<voip") {
        return None;
    }
    let raw = parse(content)
        .and_then(|doc| {
            let node = doc
                .root_element()
                .descendants()
                .skip(1)
                .find(|node| node.has_tag_name("msg"))?;
            Some(collapse(node.text().unwrap_or("")))
        })
        .unwrap_or_default();
    if raw.is_empty() {
        return Some("[通话]".into());
    }
    if let Some(duration) = raw.strip_prefix("Duration:") {
        return Some(if duration.trim().is_empty() {
            "[通话]".into()
        } else {
            format!("[通话] 通话时长 {}", duration.trim())
        });
    }
    let status = match raw.as_str() {
        "Canceled" => "已取消",
        "Line busy" => "对方忙线",
        "Already answered elsewhere" => "已在其他设备接听",
        "Declined on other device" => "已在其他设备拒接",
        "Call canceled by caller" => "主叫已取消",
        "Call not answered" | "Call wasn't answered" => "未接听",
        _ => &raw,
    };
    Some(format!("[通话] {status}"))
}

pub fn voice(content: &str) -> String {
    let duration = parse(content).and_then(|doc| {
        doc.root_element()
            .descendants()
            .skip(1)
            .find(|node| node.has_tag_name("voicemsg"))?
            .attribute("voicelength")?
            .trim()
            .parse::<u64>()
            .ok()
    });
    match duration {
        Some(ms) if ms > 0 => format!("[语音 {:.1}s]", ms as f64 / 1000.0),
        _ => "[语音]".into(),
    }
}

pub fn namecard(content: &str) -> Option<String> {
    let doc = parse(content)?;
    let root = doc.root_element();
    let nickname = root.attribute("nickname").unwrap_or("").trim();
    let username = root.attribute("username").unwrap_or("").trim();
    if nickname.is_empty() && username.is_empty() {
        return None;
    }
    let mut head = if nickname.is_empty() {
        username
    } else {
        nickname
    }
    .to_owned();
    if username.starts_with("gh_") {
        head.push_str(&format!(" (公众号 {username})"));
    }
    let bio = collapse(root.attribute("certinfo").unwrap_or(""));
    Some(if bio.is_empty() {
        format!("[名片] {head}")
    } else {
        format!("[名片] {head}: {bio}")
    })
}

pub fn location(content: &str) -> Option<String> {
    super::location::parse(content).map(|info| info.summary())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_legacy_golden() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/summary-golden.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let xml = case["xml"].as_str().unwrap();
            let actual = match case["kind"].as_str().unwrap() {
                "voice" => Some(voice(xml)),
                "namecard" => namecard(xml),
                "location" => location(xml),
                "voip" => voip(xml),
                "video" => Some(video(xml)),
                _ => unreachable!(),
            };
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                case["expected"],
                "{case}"
            );
        }
    }

    #[test]
    fn voice_duration_and_fallbacks() {
        for (ms, expected) in [("3300", "3.3"), ("800", "0.8"), ("62000", "62.0")] {
            assert_eq!(
                voice(&format!("<msg><voicemsg voicelength='{ms}'/></msg>")),
                format!("[语音 {expected}s]")
            );
        }
        for xml in [
            "",
            "<msg/>",
            "<msg>",
            "<voicemsg voicelength='1000'/>",
            "<msg><voicemsg voicelength='-1'/></msg>",
            "<msg><voicemsg voicelength='0'/></msg>",
        ] {
            assert_eq!(voice(xml), "[语音]");
        }
    }

    #[test]
    fn namecard_hides_credentials() {
        assert_eq!(namecard("<msg nickname=' 示例 ' username='gh_test' certinfo='一行&#10; 两行' antispamticket='secret'/>"), Some("[名片] 示例 (公众号 gh_test): 一行 两行".into()));
        assert_eq!(
            namecard("<msg username='wxid_test'/>"),
            Some("[名片] wxid_test".into())
        );
        assert_eq!(namecard("<msg antispamticket='secret'/>"), None);
    }

    #[test]
    fn location_placeholder_category_and_address() {
        assert_eq!(location("<msg><location poiname='公园' label='示例路' poiCategoryTips='休闲:公园' x='1' y='2'/></msg>"), Some("[位置·休闲] 公园 @ 示例路".into()));
        assert_eq!(
            location("<msg><location poiname='[Location]' label='示例路'/></msg>"),
            Some("[位置] 示例路".into())
        );
        assert_eq!(location("<msg><location/></msg>"), Some("[位置]".into()));
        assert_eq!(location("<msg/>"), None);
    }

    #[test]
    fn rejects_unsafe_and_oversized_xml() {
        for xml in [
            "<!DOCTYPE msg [<!ENTITY e 'secret'>]><msg nickname='&e;'/>",
            "<msg>",
        ] {
            assert!(parse(xml).is_none());
        }
        assert!(parse(&format!("<msg>{}</msg>", "中".repeat(19_989))).is_some());
        assert!(parse(&format!("<msg>{}</msg>", "中".repeat(19_990))).is_none());
    }
}
