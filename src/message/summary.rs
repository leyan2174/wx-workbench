//! Reading-summary projections over decoded metadata; no private XML parsing.
use crate::business::structured_message::{CallSummary, LocationSummary, NamecardSummary};

pub fn video(duration: Option<&str>) -> String {
    duration
        .map(|value| format!("[视频] {value}秒"))
        .unwrap_or_else(|| "[视频]".into())
}

pub fn voice(duration: Option<u64>) -> String {
    match duration {
        Some(ms) if ms > 0 => format!("[语音 {:.1}s]", ms as f64 / 1000.0),
        _ => "[语音]".into(),
    }
}

pub fn voip(call: &CallSummary) -> String {
    let status = match call {
        CallSummary::Empty => return "[通话]".into(),
        CallSummary::Duration(value) => return format!("[通话] 通话时长 {value}"),
        CallSummary::Canceled => "已取消",
        CallSummary::Busy => "对方忙线",
        CallSummary::AnsweredElsewhere => "已在其他设备接听",
        CallSummary::DeclinedElsewhere => "已在其他设备拒接",
        CallSummary::CanceledByCaller => "主叫已取消",
        CallSummary::NotAnswered => "未接听",
        CallSummary::Other(value) => value,
    };
    format!("[通话] {status}")
}

pub fn namecard(card: &NamecardSummary) -> String {
    let mut head = if card.nickname.is_empty() {
        &card.username
    } else {
        &card.nickname
    }
    .clone();
    if card.is_public_account {
        head.push_str(&format!(" (公众号 {})", card.username));
    }
    if card.biography.is_empty() {
        format!("[名片] {head}")
    } else {
        format!("[名片] {head}: {}", card.biography)
    }
}

pub fn location(info: &LocationSummary) -> String {
    let head = if info.category.is_empty() {
        "[位置]".to_owned()
    } else {
        format!("[位置·{}]", info.category)
    };
    let name = &info.name;
    let label = &info.address;
    if name.is_empty() || (name.starts_with('[') && name.ends_with(']')) {
        return if label.is_empty() {
            head
        } else {
            format!("{head} {label}")
        };
    }
    if label.is_empty() || name == label {
        format!("{head} {name}")
    } else {
        format!("{head} {name} @ {label}")
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::wechat::messages::summary::*;
    use crate::message::xml::parse;

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
