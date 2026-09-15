//! Legacy summary decoding, deliberately distinct from bounded rich previews.
use crate::business::structured_message::{CallSummary, NamecardSummary};
use crate::message::{
    summary as display,
    xml::{collapse, parse},
};

pub fn video_duration(content: &str) -> Option<String> {
    parse(content).and_then(|doc| {
        let node = doc
            .root_element()
            .descendants()
            .skip(1)
            .find(|node| node.has_tag_name("videomsg"))?;
        node.attribute("playlength")
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

pub fn video(content: &str) -> String {
    display::video(video_duration(content).as_deref())
}

pub fn voice_duration(content: &str) -> Option<u64> {
    parse(content).and_then(|doc| {
        doc.root_element()
            .descendants()
            .skip(1)
            .find(|node| node.has_tag_name("voicemsg"))?
            .attribute("voicelength")?
            .trim()
            .parse::<u64>()
            .ok()
    })
}

pub fn voice(content: &str) -> String {
    display::voice(voice_duration(content))
}

pub fn call(content: &str) -> Option<CallSummary> {
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
        return Some(CallSummary::Empty);
    }
    if let Some(duration) = raw.strip_prefix("Duration:") {
        return Some(if duration.trim().is_empty() {
            CallSummary::Empty
        } else {
            CallSummary::Duration(duration.trim().to_owned())
        });
    }
    Some(match raw.as_str() {
        "Canceled" => CallSummary::Canceled,
        "Line busy" => CallSummary::Busy,
        "Already answered elsewhere" => CallSummary::AnsweredElsewhere,
        "Declined on other device" => CallSummary::DeclinedElsewhere,
        "Call canceled by caller" => CallSummary::CanceledByCaller,
        "Call not answered" | "Call wasn't answered" => CallSummary::NotAnswered,
        _ => CallSummary::Other(raw),
    })
}

pub fn voip(content: &str) -> Option<String> {
    call(content).as_ref().map(display::voip)
}

pub fn namecard_info(content: &str) -> Option<NamecardSummary> {
    let doc = parse(content)?;
    let root = doc.root_element();
    let nickname = root.attribute("nickname").unwrap_or("").trim();
    let username = root.attribute("username").unwrap_or("").trim();
    if nickname.is_empty() && username.is_empty() {
        return None;
    }
    Some(NamecardSummary {
        nickname: nickname.to_owned(),
        username: username.to_owned(),
        is_public_account: username.starts_with("gh_"),
        biography: collapse(root.attribute("certinfo").unwrap_or("")),
    })
}

pub fn namecard(content: &str) -> Option<String> {
    namecard_info(content).as_ref().map(display::namecard)
}

pub fn location(content: &str) -> Option<String> {
    super::location::parse(content).map(|info| display::location(&info.summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_duration_and_call_status_remain_text() {
        let xml = "<msg><videomsg playlength=' next-format '/></msg>";
        assert_eq!(video_duration(xml).as_deref(), Some(" next-format "));
        assert_eq!(video(xml), "[视频]  next-format 秒");
        assert_eq!(video("<videomsg playlength='12'/>"), "[视频]");
        assert_eq!(
            voice_duration("<msg><voicemsg voicelength='1_000'/></msg>"),
            None
        );
        assert_eq!(
            call("<voipmsg><msg>New status</msg></voipmsg>"),
            Some(CallSummary::Other("New status".into()))
        );
        assert_eq!(
            voip("<voipmsg><msg>New status</msg></voipmsg>"),
            Some("[通话] New status".into())
        );
        assert_eq!(
            call("<voipmsg><msg>Duration: next</msg></voipmsg>"),
            Some(CallSummary::Duration("next".into()))
        );
        assert_eq!(voip("<voip-broken"), Some("[通话]".into()));
        assert_eq!(voip("ordinary text"), None);
    }

    #[test]
    fn typed_cards_omit_credentials_and_location_keeps_coordinate_direction() {
        let card = namecard_info(
            "<msg nickname=' Name ' username='gh_test' certinfo=' bio ' antispamticket='secret'/>",
        )
        .unwrap();
        assert_eq!(
            card,
            NamecardSummary {
                nickname: "Name".into(),
                username: "gh_test".into(),
                is_public_account: true,
                biography: "bio".into(),
            }
        );
        let place = super::super::location::parse("<msg><location x='12.5' y='99.5' poiname='[Location]' label='Road' poiCategoryTips='Park:Other'/></msg>").unwrap();
        assert_eq!(place.latitude, Some(12.5));
        assert_eq!(place.longitude, Some(99.5));
        assert_eq!(display::location(&place.summary), "[位置·Park] Road");
        assert_eq!(location("<location/>"), None);
    }
}
