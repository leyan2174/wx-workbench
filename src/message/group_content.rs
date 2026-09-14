//! Shared group sender-prefix parsing without storage or export dependencies.

/// 返回群消息内的发送者及正文；仅认可旧格式允许的账号字符和 XML 前缀。
pub fn split_group_content(content: &str) -> (&str, &str) {
    if let Some(parts) = content.split_once(":\n") {
        return parts;
    }
    if let Some((sender, body)) = content.split_once(':') {
        let valid_sender = !sender.is_empty()
            && sender
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-@.".contains(&b));
        if valid_sender
            && ["<?xml", "<msg", "<msglist", "<voipmsg", "<sysmsg"]
                .iter()
                .any(|prefix| body.starts_with(prefix))
        {
            return (sender, body);
        }
    }
    ("", content)
}

#[cfg(test)]
mod group_tests {
    use super::split_group_content;

    #[test]
    fn supports_legacy_xml_prefixes_without_eating_plain_text() {
        for body in [
            "<msg/>",
            "<?xml version='1.0'?><msg/>",
            "<msglist/>",
            "<voipmsg/>",
            "<sysmsg/>",
        ] {
            let xml = format!("wxid_demo-1:{body}");
            assert_eq!(split_group_content(&xml), ("wxid_demo-1", body));
        }
        assert_eq!(
            split_group_content("wxid_demo:\n正文"),
            ("wxid_demo", "正文")
        );
        for text in [
            "https://example.test",
            "说明:正文",
            "a b:<msg/>",
            ":<msg/>",
            "wxid_demo:<unknown/>",
        ] {
            assert_eq!(split_group_content(text), ("", text));
        }
    }
}
