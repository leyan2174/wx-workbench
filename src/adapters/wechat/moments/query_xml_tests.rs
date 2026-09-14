use super::*;

fn make_post_xml(
    create_time: &str,
    desc: &str,
    username_tag: Option<&str>,
    media: usize,
    location: Option<&str>,
) -> String {
    let username = username_tag
        .map(|u| format!("<username>{}</username>", u))
        .unwrap_or_default();
    let media_tags = "<media><type>2</type></media>".repeat(media);
    let content_object = if media > 0 {
        format!(
            "<ContentObject><mediaList>{}</mediaList></ContentObject>",
            media_tags
        )
    } else {
        String::new()
    };
    let loc = location
        .map(|p| format!(r#"<location poiName="{}" longitude="0" latitude="0" />"#, p))
        .unwrap_or_default();
    format!(
        "<TimelineObject>{}<createTime>{}</createTime><contentDesc>{}</contentDesc>{}{}</TimelineObject>",
        username, create_time, desc, content_object, loc
    )
}

#[test]
fn parse_uses_user_name_column_when_present() {
    let xml = make_post_xml("1700000000", "hello", Some("wxid_xml"), 0, None);
    let p = parse_post_xml(1, "wxid_column", &xml);
    assert_eq!(p.author_username, "wxid_column");
    assert_eq!(p.create_time, 1700000000);
    assert_eq!(p.content, "hello");
    assert_eq!(p.media.len(), 0);
    assert_eq!(p.location, "");
}

#[test]
fn parse_falls_back_to_xml_username_when_column_empty() {
    let xml = make_post_xml("1700000001", "world", Some("wxid_xml_only"), 0, None);
    let p = parse_post_xml(2, "", &xml);
    assert_eq!(p.author_username, "wxid_xml_only");
}

#[test]
fn parse_handles_missing_create_time() {
    let xml = "<TimelineObject><contentDesc>x</contentDesc></TimelineObject>";
    let p = parse_post_xml(3, "wxid", xml);
    assert_eq!(p.create_time, 0);
    assert_eq!(p.content, "x");
}

#[test]
fn parse_counts_media_and_extracts_location() {
    let xml = make_post_xml("1700000002", "post", None, 3, Some("Wuxi"));
    let p = parse_post_xml(4, "wxid", &xml);
    assert_eq!(p.media.len(), 3);
    assert_eq!(p.location, "Wuxi");
}

#[test]
fn parse_when_both_column_and_xml_username_empty_returns_empty_author() {
    let xml = "<TimelineObject><createTime>1700000003</createTime><contentDesc>orphan</contentDesc></TimelineObject>";
    let p = parse_post_xml(5, "", xml);
    assert_eq!(p.author_username, "");
}

#[test]
fn parse_decodes_xml_entities_in_content() {
    // 单 DOM 解析的副作用：roxmltree 自动把 &lt; / &amp; / &quot; 等还原成原字符；
    // 旧版 extract_xml_text 字符串扫描不解码，会把 "&lt;world&gt;" 原样输出。
    // 新版语义对下游更正确（拿到的就是用户真实内容），把这个行为锁进测试。
    let xml = "<TimelineObject><contentDesc>Hello &lt;world&gt; &amp; friends</contentDesc></TimelineObject>";
    let p = parse_post_xml(6, "wxid", xml);
    assert_eq!(p.content, "Hello <world> & friends");
}

#[test]
fn parse_malformed_xml_falls_back_to_string_fields_when_column_present() {
    let xml = "<TimelineObject><createTime>1700000007</createTime><contentDesc>A &amp; B</contentDesc><location poiName=\"Wuxi &amp; Lake\" /><not valid xml";
    let p = parse_post_xml(7, "wxid_fallback", xml);
    assert_eq!(p.create_time, 1700000007);
    assert_eq!(p.content, "A & B");
    assert_eq!(p.author_username, "wxid_fallback");
    assert!(p.media.is_empty());
    assert_eq!(p.location, "Wuxi & Lake");
}

#[test]
fn parse_malformed_xml_can_still_use_xml_username_when_column_empty() {
    let xml = "<TimelineObject><createTime>1700000008</createTime><contentDesc>broken</contentDesc><username>wxid_xml_only</username><not valid xml";
    let p = parse_post_xml(8, "", xml);
    assert_eq!(p.create_time, 1700000008);
    assert_eq!(p.content, "broken");
    assert_eq!(p.author_username, "wxid_xml_only");
    assert!(p.media.is_empty());
}

#[test]
fn parse_without_timeline_object_falls_back_to_string_fields() {
    let xml = "<SnsDataItem><createTime>1700000009</createTime><contentDesc>still here</contentDesc><username>wxid_outer</username></SnsDataItem>";
    let p = parse_post_xml(9, "", xml);
    assert_eq!(p.create_time, 1700000009);
    assert_eq!(p.content, "still here");
    assert_eq!(p.author_username, "wxid_outer");
    assert!(p.media.is_empty());
}

#[test]
fn escape_like_pattern_escapes_backslash_first() {
    // 反斜杠必须在 % / _ 之前转义；否则后面塞进去的 \% / \_ 会被再次双转义吃掉
    assert_eq!(escape_like_pattern("a\\b"), "a\\\\b");
    assert_eq!(escape_like_pattern("100%"), "100\\%");
    assert_eq!(escape_like_pattern("foo_bar"), "foo\\_bar");
}

#[test]
fn escape_like_pattern_combined() {
    // \%_ 三个元字符同时出现
    let escaped = escape_like_pattern("a\\b%c_d");
    assert_eq!(escaped, "a\\\\b\\%c\\_d");
}

#[test]
fn escape_like_pattern_no_special_chars_unchanged() {
    assert_eq!(escape_like_pattern("hello world"), "hello world");
    assert_eq!(escape_like_pattern("中文关键词"), "中文关键词");
    assert_eq!(escape_like_pattern(""), "");
}

fn media_object(value: &Value) -> &serde_json::Map<String, Value> {
    value.as_object().expect("media entry should be an object")
}

#[test]
fn single_image_media() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <mediaList>
    <media>
      <type>2</type>
      <url enc_idx="1" key="placeholder-key" token="placeholder-token" md5="placeholder-md5">https://szmmsns.qpic.cn/&lt;redacted&gt;/image.jpg</url>
      <thumb enc_idx="0" key="placeholder-thumb-key" token="placeholder-thumb-token">https://szmmsns.qpic.cn/&lt;redacted&gt;/thumb.jpg</thumb>
      <size width="1440" height="1080" totalSize="123456" />
    </media>
  </mediaList>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    let media = parse_post_media(xml);
    assert_eq!(media.len(), 1);

    let item = media_object(&media[0]);
    assert_eq!(item.get("type").and_then(Value::as_str), Some("2"));
    assert_eq!(
        item.get("url").and_then(Value::as_str),
        Some("https://szmmsns.qpic.cn/<redacted>/image.jpg")
    );
    assert_eq!(
        item.get("thumb").and_then(Value::as_str),
        Some("https://szmmsns.qpic.cn/<redacted>/thumb.jpg")
    );
    assert_eq!(item.get("url_enc_idx").and_then(Value::as_str), Some("1"));
    assert_eq!(
        item.get("url_key").and_then(Value::as_str),
        Some("placeholder-key")
    );
    assert_eq!(
        item.get("url_token").and_then(Value::as_str),
        Some("placeholder-token")
    );
    assert_eq!(
        item.get("md5").and_then(Value::as_str),
        Some("placeholder-md5")
    );
    assert_eq!(item.get("width").and_then(Value::as_i64), Some(1440));
    assert_eq!(item.get("height").and_then(Value::as_i64), Some(1080));
    assert_eq!(item.get("total_size").and_then(Value::as_i64), Some(123456));
}

#[test]
fn three_images_media() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <mediaList>
    <media>
      <type>2</type>
      <sub_type>10</sub_type>
      <url enc_idx="1" key="placeholder-key-1" token="placeholder-token-1">https://szmmsns.qpic.cn/&lt;redacted&gt;/image-1.jpg</url>
      <thumb>https://szmmsns.qpic.cn/&lt;redacted&gt;/thumb-1.jpg</thumb>
      <size width="100" height="200" totalSize="111" />
    </media>
    <media>
      <type>2</type>
      <sub_type>11</sub_type>
      <url enc_idx="0" key="placeholder-key-2" token="placeholder-token-2">https://szmmsns.qpic.cn/&lt;redacted&gt;/image-2.jpg</url>
      <thumb>https://szmmsns.qpic.cn/&lt;redacted&gt;/thumb-2.jpg</thumb>
      <size width="300" height="400" totalSize="222" />
    </media>
    <media>
      <type>6</type>
      <url>https://szmmsns.qpic.cn/&lt;redacted&gt;/image-3.jpg</url>
      <thumb enc_idx="1" key="placeholder-thumb-key-3" token="placeholder-thumb-token-3">https://szmmsns.qpic.cn/&lt;redacted&gt;/thumb-3.jpg</thumb>
      <size width="500" height="600" totalSize="333" />
    </media>
  </mediaList>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    let media = parse_post_media(xml);
    assert_eq!(media.len(), 3);

    let first = media_object(&media[0]);
    assert_eq!(first.get("sub_type").and_then(Value::as_str), Some("10"));
    assert_eq!(
        first.get("url_key").and_then(Value::as_str),
        Some("placeholder-key-1")
    );

    let second = media_object(&media[1]);
    assert_eq!(second.get("sub_type").and_then(Value::as_str), Some("11"));
    assert_eq!(second.get("width").and_then(Value::as_i64), Some(300));

    let third = media_object(&media[2]);
    assert_eq!(third.get("type").and_then(Value::as_str), Some("6"));
    assert_eq!(
        third.get("thumb_key").and_then(Value::as_str),
        Some("placeholder-thumb-key-3")
    );
}

#[test]
fn video_media() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<id>18446744073709551610</id>
<ContentObject>
  <mediaList>
    <media>
      <id>12345678901234567890</id>
      <type>15</type>
      <url enc_idx="1" key="placeholder-video-key" token="placeholder-video-token">https://szmmsns.qpic.cn/&lt;redacted&gt;/video.mp4</url>
      <thumb>https://szmmsns.qpic.cn/&lt;redacted&gt;/video-thumb.jpg</thumb>
      <size width="720" height="1280" />
      <videomd5>&lt;placeholder-video-md5&gt;</videomd5>
      <videoDuration>37</videoDuration>
      <enc key="9876543210">1</enc>
    </media>
  </mediaList>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    let media = parse_post_media(xml);
    assert_eq!(media.len(), 1);

    let item = media_object(&media[0]);
    assert_eq!(
        item.get("video_md5").and_then(Value::as_str),
        Some("<placeholder-video-md5>")
    );
    assert_eq!(item.get("video_duration").and_then(Value::as_i64), Some(37));
    assert_eq!(
        item.get("id").and_then(Value::as_str),
        Some("12345678901234567890")
    );
    assert_eq!(
        item.get("enc_key").and_then(Value::as_str),
        Some("9876543210")
    );
    assert!(!item.contains_key("total_size"));
}

#[test]
fn parse_exposes_timeline_post_id() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<id>18446744073709551610</id>
<createTime>1700000000</createTime>
<contentDesc>hello</contentDesc>
  </TimelineObject>
</SnsDataItem>
    "#;

    let post = parse_post_xml(-6, "wxid_test", xml);
    assert_eq!(post.post_id, "18446744073709551610");
}

#[test]
fn text_only_post() {
    let without_media_list = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <type>1</type>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;
    let empty_media_list = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <mediaList />
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    assert!(parse_post_media(without_media_list).is_empty());
    assert!(parse_post_media(empty_media_list).is_empty());
}

#[test]
fn malformed_xml() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <mediaList>
    <media>
      <type>2</type>
  </mediaList>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    assert!(parse_post_media(xml).is_empty());
}

#[test]
fn size_without_total_size_omits_total_size_key() {
    let xml = r#"
<SnsDataItem>
  <TimelineObject>
<ContentObject>
  <mediaList>
    <media>
      <type>2</type>
      <size width="640" height="480" />
    </media>
  </mediaList>
</ContentObject>
  </TimelineObject>
</SnsDataItem>
    "#;

    let media = parse_post_media(xml);
    assert_eq!(media.len(), 1);
    let item = media_object(&media[0]);
    assert_eq!(item.get("width").and_then(Value::as_i64), Some(640));
    assert_eq!(item.get("height").and_then(Value::as_i64), Some(480));
    assert!(!item.contains_key("total_size"));
}
