//! WeChat structured message decoding; metadata only, never resolves media.
use super::messages::{export_content::refer_summary, transfer};
use crate::business::structured_message::{
    ChatItem, ContentIssue, StructuredMessage as RichMessage,
};
use crate::message::{split_group_content, xml};
use roxmltree::Node;

const MAX_INPUT_BYTES: usize = 131_072;
const MAX_TITLE: usize = 512;
const MAX_URL_BYTES: usize = 4096;
const MAX_ITEMS: usize = 20;
// JSON consumers must not lose integer precision.
const MAX_JSON_INTEGER: u64 = 9_007_199_254_740_991;

/// Decode metadata without conflating an unsupported kind with malformed input.
/// Existing projections may retain their ordinary summary for any preview issue.
pub(crate) fn decode(
    local_type: i64,
    content: &str,
    is_group: bool,
) -> Result<RichMessage, ContentIssue> {
    if content.len() > MAX_INPUT_BYTES {
        return Err(ContentIssue::InputTooLarge);
    }
    let base = local_type & 0xffff_ffff;
    if local_type < 0 || !matches!(base, 34 | 43 | 47 | 49) {
        return Err(ContentIssue::UnsupportedKind);
    }
    let body = if is_group {
        split_group_content(content).1
    } else {
        content
    }
    .trim();
    let doc = xml::parse(body).ok_or(ContentIssue::MalformedContent)?;
    let root = doc.root_element();
    let preview = (|| {
        let rich = match base {
            49 => parse_app(body, message_node(root, "appmsg")?, local_type >> 32)?,
            34 => {
                let voice = message_node(root, "voicemsg")?;
                let millis = integer(voice.attribute("voicelength")?)?;
                // Same one-decimal conversion as the native voice summary.
                let duration = format!("{:.1}", millis as f64 / 1000.0).parse().ok()?;
                RichMessage::Voice { duration }
            }
            43 => RichMessage::Video {
                duration: integer(message_node(root, "videomsg")?.attribute("playlength")?)?,
            },
            47 => {
                let emoji = message_node(root, "emoji")?;
                let emoji_url = ["thumburl", "externurl", "cdnurl"]
                    .into_iter()
                    .filter_map(|key| emoji.attribute(key))
                    .find_map(|value| safe_url(value, false))
                    .unwrap_or_default();
                let md5 = emoji
                    .attribute("md5")
                    .filter(|value| {
                        value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if emoji_url.is_empty() && md5.is_empty() {
                    return None;
                }
                RichMessage::Emoji { emoji_url, md5 }
            }
            _ => return None,
        };
        Some(rich)
    })();
    preview.ok_or(ContentIssue::NoSafePreview)
}

// Only the message root or its immediate payload may select a card type; a
// nested quoted/forwarded payload must not masquerade as the outer message.
fn message_node<'a, 'input>(root: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    if root.has_tag_name(tag) && root.tag_name().namespace().is_none() {
        Some(root)
    } else if root.has_tag_name("msg") && root.tag_name().namespace().is_none() {
        child(root, tag)
    } else {
        None
    }
}

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    let mut matches = node
        .children()
        .filter(|n| n.has_tag_name(tag) && n.tag_name().namespace().is_none());
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn text<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> &'a str {
    child(node, tag)
        .filter(|n| !n.children().any(|n| n.is_element()))
        .and_then(|n| n.text())
        .unwrap_or_default()
}

// Keep text, never serialized XML/HTML from an escaped payload. The UI still
// must render every field as text, and must not automatically fetch media URLs.
fn plain(value: &str, limit: usize) -> String {
    if value.contains(['<', '>']) || value.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return String::new();
    }
    value.trim().chars().take(limit).collect()
}

fn field(node: Node<'_, '_>, tag: &str, limit: usize) -> String {
    plain(text(node, tag), limit)
}

fn integer(value: &str) -> Option<u64> {
    let value = value.trim().strip_prefix('+').unwrap_or(value.trim());
    if value.is_empty()
        || !value
            .split('_')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    value
        .replace('_', "")
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= MAX_JSON_INTEGER)
}

fn safe_url(value: &str, article: bool) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_URL_BYTES
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || value.contains(['<', '>', '\\'])
    {
        return None;
    }
    let mut url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    if article && url.host_str() == Some("mp.weixin.qq.com") {
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, val)| {
                !val.is_empty() && matches!(key.as_ref(), "__biz" | "mid" | "idx" | "sn" | "chksm")
            })
            .map(|(key, val)| (key.into_owned(), val.into_owned()))
            .collect();
        url.set_query(None);
        if !kept.is_empty() {
            url.query_pairs_mut().extend_pairs(kept);
        }
        url.set_fragment(None);
    }
    Some(url.into())
}

fn parse_app(body: &str, app: Node<'_, '_>, subtype: i64) -> Option<RichMessage> {
    // An explicit malformed type is not equivalent to an absent legacy type.
    let types: Vec<_> = app.children().filter(|n| n.has_tag_name("type")).collect();
    let kind = match types.as_slice() {
        [] => u64::try_from(subtype).ok()?,
        [node] if node.tag_name().namespace().is_none() => integer(text(app, "type"))?,
        _ => return None,
    };
    let title = field(app, "title", MAX_TITLE);
    let des = field(app, "des", 200);
    let source = field(app, "sourcedisplayname", 200);
    match kind {
        57 => {
            let refer = child(app, "refermsg")?;
            let ref_type = integer(text(refer, "type"))?.to_string();
            let summary = refer_summary(&ref_type, text(refer, "content"));
            let ref_content = plain(&summary, 100);
            let ref_name = field(refer, "displayname", 200);
            if title.is_empty() && ref_content.is_empty() {
                return None;
            }
            Some(RichMessage::Quote {
                title,
                ref_name,
                ref_content,
            })
        }
        6 => {
            let attach = child(app, "appattach");
            let file_ext = attach.map(|n| field(n, "fileext", 32)).unwrap_or_default();
            let size = attach.map(|n| text(n, "totallen")).unwrap_or_default();
            let file_size = if size.trim().is_empty() {
                0
            } else {
                integer(size)?
            };
            if title.is_empty() && file_ext.is_empty() && file_size == 0 {
                return None;
            }
            Some(RichMessage::File {
                title,
                file_ext,
                file_size,
            })
        }
        33 | 36 => {
            let url = safe_url(text(app, "url"), false).unwrap_or_default();
            if title.is_empty() && source.is_empty() && url.is_empty() {
                return None;
            }
            Some(RichMessage::Miniapp { title, source, url })
        }
        51 => (!title.is_empty()).then_some(RichMessage::Channels { title }),
        19 => {
            let mut items = Vec::new();
            let record = text(app, "recorditem");
            if !record.trim().is_empty() {
                // One bounded nested document, never recursive record decoding.
                let doc = xml::parse(record.trim())?;
                for item in doc
                    .descendants()
                    .filter(|n| n.has_tag_name("dataitem") && n.tag_name().namespace().is_none())
                {
                    let name = field(item, "sourcename", 200);
                    let text = field(item, "datadesc", 100);
                    if !name.is_empty() && !text.is_empty() {
                        items.push(ChatItem { name, text });
                    }
                    if items.len() >= MAX_ITEMS {
                        break;
                    }
                }
            }
            if title.is_empty() && des.is_empty() && items.is_empty() {
                return None;
            }
            Some(RichMessage::Chatlog { title, des, items })
        }
        2000 => {
            child(app, "wcpayinfo")?;
            // Shared parser owns transfer spelling/case quirks and status decoding.
            // Its unbounded XML entry is used only after the bounded outer parse.
            let parsed = transfer::parse(body).ok()?;
            let info = parsed.details;
            let raw_subtype = plain(&info.raw_subtype, 32);
            let amount_text = plain(&info.amount_text, 100);
            let memo = plain(&info.memo, 200);
            if title.is_empty()
                && raw_subtype.is_empty()
                && amount_text.is_empty()
                && memo.is_empty()
            {
                return None;
            }
            Some(RichMessage::Transfer {
                title,
                status: info.status,
                raw_subtype,
                amount_text,
                memo,
            })
        }
        _ => {
            let url = safe_url(text(app, "url"), kind == 5).unwrap_or_default();
            if title.is_empty() && des.is_empty() && url.is_empty() && source.is_empty() {
                return None;
            }
            Some(RichMessage::Link {
                title,
                des,
                url,
                source,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn numeric_safe_previews_do_not_change_legacy_summary_or_transfer_detail_policy() {
        use crate::adapters::wechat::messages::{summary, transfer};
        let video = "<msg><videomsg playlength='next-format'/></msg>";
        assert_eq!(summary::video(video), "[视频] next-format秒");
        assert!(super::decode(43, video, false).is_err());
        let voice = "<msg><voicemsg voicelength='1_000'/></msg>";
        assert_eq!(summary::voice(voice), "[语音]");
        assert_eq!(
            super::decode(34, voice, false).unwrap(),
            super::RichMessage::Voice { duration: 1.0 }
        );
        let xml = "<msg><appmsg><type>2000</type><wcpayinfo><paysubtype>99</paysubtype><feedesc>0.010 CNY</feedesc></wcpayinfo></appmsg></msg>";
        let details = transfer::parse(xml).unwrap();
        assert_eq!(
            crate::message::transfer::status_label(&details.details),
            "未知(paysubtype=99)"
        );
        let super::RichMessage::Transfer {
            status,
            amount_text,
            ..
        } = super::decode(49, xml, false).unwrap()
        else {
            panic!("expected transfer")
        };
        assert_eq!(
            status,
            crate::business::structured_message::TransferStatus::Unknown
        );
        assert_eq!(amount_text, "0.010 CNY");
    }

    use super::*;
    use serde_json::{json, Value};

    fn parse(local_type: i64, content: &str, is_group: bool) -> Option<Value> {
        super::decode(local_type, content, is_group)
            .ok()
            .map(|message| crate::message::structured_message::project(&message))
    }

    #[test]
    fn preview_issues_keep_distinct_causes_and_do_not_guess_call_media() {
        assert_eq!(
            super::decode(50, "<voip><msg>Video call</msg></voip>", false),
            Err(ContentIssue::UnsupportedKind)
        );
        assert_eq!(
            super::decode(49, "<msg>", false),
            Err(ContentIssue::MalformedContent)
        );
        assert_eq!(
            super::decode(34, "<msg><voicemsg/></msg>", false),
            Err(ContentIssue::NoSafePreview)
        );
        assert_eq!(
            super::decode(49, &"x".repeat(MAX_INPUT_BYTES + 1), false),
            Err(ContentIssue::InputTooLarge)
        );
        assert!(matches!(
            super::decode(34, "<msg><voicemsg voicelength='1000'/></msg>", false),
            Ok(RichMessage::Voice { duration: 1.0 })
        ));
    }

    fn app(kind: u64, body: &str) -> String {
        format!("<msg><appmsg><type>{kind}</type>{body}</appmsg></msg>")
    }

    fn rich(kind: u64, body: &str) -> Value {
        parse(49, &app(kind, body), false).expect("synthetic rich message")
    }

    #[test]
    fn link_fields_and_article_url_cleanup() {
        assert_eq!(rich(5, "<title>Article &amp; title</title><des>Preview</des><sourcedisplayname>Publisher</sourcedisplayname><url>https://mp.weixin.qq.com/s?__biz=abc&amp;mid=1&amp;mid=2&amp;idx=1&amp;sn=s&amp;chksm=c&amp;scene=99#tracking</url>"), json!({
            "type":"link", "title":"Article & title", "des":"Preview", "source":"Publisher",
            "url":"https://mp.weixin.qq.com/s?__biz=abc&mid=1&mid=2&idx=1&sn=s&chksm=c"
        }));
        assert_eq!(
            rich(
                5,
                "<title>A</title><url>https://mp.weixin.qq.com.evil.example/s?scene=99#x</url>"
            )["url"],
            "https://mp.weixin.qq.com.evil.example/s?scene=99#x"
        );
    }

    #[test]
    fn file_miniapp_channels_and_unknown_cards_have_actual_fields() {
        assert_eq!(rich(6, "<title>report.pdf</title><appattach><totallen>2_048</totallen><fileext>pdf</fileext></appattach>"),
            json!({"type":"file","title":"report.pdf","file_ext":"pdf","file_size":2048}));
        for kind in [33, 36] {
            assert_eq!(rich(kind, "<title>Order</title><sourcedisplayname>Store</sourcedisplayname><url>https://example.com/order</url>"),
                json!({"type":"miniapp","title":"Order","source":"Store","url":"https://example.com/order"}));
        }
        assert_eq!(
            rich(51, "<title>Clip title</title>"),
            json!({"type":"channels","title":"Clip title"})
        );
        assert_eq!(
            rich(123, "<title>Other card</title>")["title"],
            "Other card"
        );
    }

    #[test]
    fn chatlog_cdata_has_bounded_real_previews() {
        let rows = (0..25).map(|i| format!("<dataitem><sourcename>User{i}</sourcename><datadesc>Message{i}</datadesc></dataitem>")).collect::<String>();
        let value = rich(19, &format!("<title>History</title><des>Summary</des><recorditem><![CDATA[<recordinfo><datalist>{rows}</datalist></recordinfo>]]></recorditem>"));
        assert_eq!(value["type"], "chatlog");
        assert_eq!(value["title"], "History");
        assert_eq!(value["des"], "Summary");
        assert_eq!(value["items"].as_array().unwrap().len(), 20);
        assert_eq!(
            value["items"][19],
            json!({"name":"User19","text":"Message19"})
        );
        assert!(parse(
            49,
            &app(
                19,
                "<title>T</title><recorditem>&lt;broken&gt;</recorditem>"
            ),
            false
        )
        .is_none());
    }

    #[test]
    fn quote_uses_native_summary_without_nested_xml_or_cdn_secrets() {
        let value = rich(57, "<title>Reply</title><refermsg><type>49</type><displayname>Alice</displayname><content>&lt;msg&gt;&lt;appmsg&gt;&lt;type&gt;6&lt;/type&gt;&lt;title&gt;report.pdf&lt;/title&gt;&lt;cdnurl&gt;secret&lt;/cdnurl&gt;&lt;/appmsg&gt;&lt;/msg&gt;</content></refermsg>");
        assert_eq!(value["type"], "quote");
        assert_eq!(value["title"], "Reply");
        assert_eq!(value["ref_name"], "Alice");
        assert!(value["ref_content"]
            .as_str()
            .unwrap()
            .contains("report.pdf"));
        assert!(!value.to_string().contains("secret"));
        assert!(!value.to_string().contains('<'));
        assert_eq!(
            rich(
                57,
                "<title>R</title><refermsg><type>1</type><content>Plain text</content></refermsg>"
            )["ref_content"],
            "Plain text"
        );
        let value = rich(57, "<title>R</title><refermsg><type>1</type><content>&lt;msg secret='x'/&gt;</content></refermsg>");
        assert_eq!(value["ref_content"], "");
    }

    #[test]
    fn transfer_reuses_aliases_and_preserves_amount_text() {
        assert_eq!(rich(2000, "<title>Payment</title><wcpayinfo><paysubtype>3</paysubtype><feeDesc>USD 12.30</feeDesc><paymemo>Lunch</paymemo><transferid>not-exposed</transferid></wcpayinfo>"),
            json!({"type":"transfer","title":"Payment","direction":"\u{5df2}\u{6536}\u{6b3e}","paysubtype":"3","fee_desc":"USD 12.30","pay_memo":"Lunch"}));
        let value = rich(2000, "<wcpayinfo><paysubtype>99</paysubtype><feedesc>12.30</feedesc><pay_memo>Note</pay_memo></wcpayinfo>");
        assert_eq!(value["direction"], "");
        assert_eq!(value["paysubtype"], "99");
        assert_eq!(value["fee_desc"], "12.30");
        assert_eq!(value["pay_memo"], "Note");
        assert!(parse(49, &app(2000, "<title>Payment</title>"), false).is_none());
    }

    #[test]
    fn voice_video_and_emoji_are_metadata_only() {
        assert_eq!(
            parse(34, "<msg><voicemsg voicelength='3300'/></msg>", false),
            Some(json!({"type":"voice","duration":3.3}))
        );
        for (millis, duration) in [(2150, 2.1), (3350, 3.4), (1250, 1.2), (1350, 1.4)] {
            assert_eq!(
                parse(
                    34,
                    &format!("<msg><voicemsg voicelength='{millis}'/></msg>"),
                    false
                )
                .unwrap()["duration"],
                json!(duration)
            );
        }
        assert_eq!(
            parse(43, "<msg><videomsg playlength='12'/></msg>", false),
            Some(json!({"type":"video","duration":12}))
        );
        assert_eq!(parse(47, "<msg><emoji thumburl='javascript:bad' externurl='https://example.com/a.gif?a=1&amp;b=2' md5='0123456789ABCDEF0123456789ABCDEF'/></msg>", false),
            Some(json!({"type":"emoji","emoji_url":"https://example.com/a.gif?a=1&b=2","md5":"0123456789abcdef0123456789abcdef"})));
        let value = parse(
            47,
            "<msg><emoji md5='0123456789abcdef0123456789abcdef'/></msg>",
            false,
        )
        .unwrap();
        assert_eq!(value["emoji_url"], "");
        assert!(!value.to_string().contains("/img/"));
    }

    #[test]
    fn group_prefix_and_packed_type_only_apply_to_outer_payload() {
        let body = "<msg><appmsg><title>a.txt</title><appattach><totallen>8</totallen></appattach></appmsg></msg>";
        let packed = (6_i64 << 32) | 49;
        assert_eq!(parse(packed, body, false).unwrap()["type"], "file");
        for prefix in ["wxid_synthetic:\n", "wxid_synthetic:"] {
            assert_eq!(
                parse(packed, &format!("{prefix}{body}"), true).unwrap()["file_size"],
                8
            );
            assert!(parse(packed, &format!("{prefix}{body}"), false).is_none());
        }
        assert_eq!(
            parse(packed, &app(51, "<title>Actual XML kind</title>"), false).unwrap()["type"],
            "channels"
        );
        assert!(parse(1, &app(5, "<title>Not a card</title>"), false).is_none());
        assert!(parse(
            49,
            "<msg><wrapper><appmsg><type>5</type><title>Nested</title></appmsg></wrapper></msg>",
            false
        )
        .is_none());
    }

    #[test]
    fn malformed_unsafe_and_empty_payloads_return_none() {
        for body in [
            "",
            "<msg>",
            "<!DOCTYPE msg [<!ENTITY x 'bad'>]><msg/>",
            "<msg><appmsg><type>5</type><type>6</type><title>X</title></appmsg></msg>",
            "<msg><appmsg><type>nope</type><title>X</title></appmsg></msg>",
        ] {
            assert!(parse(49, body, false).is_none(), "{body}");
        }
        for kind in [5, 6, 19, 33, 36, 51, 57, 2000] {
            assert!(parse(49, &app(kind, ""), false).is_none());
        }
        assert!(parse(
            49,
            &app(5, &format!("<title>{}</title>", "x".repeat(20_001))),
            false
        )
        .is_none());
        assert!(parse(49, &"x".repeat(MAX_INPUT_BYTES + 1), true).is_none());
        assert!(parse(47, "<msg><emoji md5='invalid'/></msg>", false).is_none());
        for value in [
            "-1",
            "NaN",
            "1.2",
            "18446744073709551616",
            "9007199254740992",
            "1__0",
        ] {
            assert!(parse(
                34,
                &format!("<msg><voicemsg voicelength='{value}'/></msg>"),
                false
            )
            .is_none());
            assert!(parse(
                43,
                &format!("<msg><videomsg playlength='{value}'/></msg>"),
                false
            )
            .is_none());
            assert!(parse(
                49,
                &app(
                    6,
                    &format!(
                        "<title>File</title><appattach><totallen>{value}</totallen></appattach>"
                    )
                ),
                false
            )
            .is_none());
        }
        assert!(parse(34, "<msg><voicemsg/></msg>", false).is_none());
    }

    #[test]
    fn unsafe_urls_and_embedded_markup_never_escape_as_rich_fields() {
        for value in [
            "javascript:alert(1)",
            "file:///C:/secret",
            "data:text/plain,secret",
            "//example.com/a",
            "https://user:pass@example.com/a",
        ] {
            assert!(safe_url(value, false).is_none());
        }
        let value = rich(5, "<title>Safe</title><des>&lt;msg&gt;secret&lt;/msg&gt;</des><url>javascript:alert(1)</url>");
        assert_eq!(value["des"], "");
        assert_eq!(value["url"], "");
        assert_eq!(
            rich(
                5,
                &format!(
                    "<title>{}</title><des>{}</des>",
                    "A".repeat(600),
                    "B".repeat(300)
                )
            )["title"]
                .as_str()
                .unwrap()
                .len(),
            MAX_TITLE
        );
    }
}
