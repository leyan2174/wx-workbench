use super::*;
use serde_json::json;

fn fixtures() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if manifest.ends_with("sns-album-render") {
        manifest.into()
    } else {
        manifest.join("tests/fixtures/sns-album-render")
    }
}

#[test]
fn migrated_render_golden_is_static() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/sns-album-render/golden.json"
    ))
    .unwrap();
    for c in golden["values"].as_array().unwrap() {
        assert_eq!(clean_text(&c["value"]), c["clean"].as_str().unwrap());
        assert_eq!(render_text(&c["value"]), c["text"].as_str().unwrap());
        assert_eq!(
            safe_stem(&c["value"], "fallback"),
            c["stem"].as_str().unwrap()
        );
    }
    for c in golden["emoji"].as_array().unwrap() {
        assert_eq!(
            render_text(&json!(format!("[{}]", c["name"].as_str().unwrap()))),
            c["text"].as_str().unwrap()
        );
    }
    for c in golden["cases"].as_array().unwrap() {
        let result = render("合成<&\"'", c["posts"].as_array().unwrap()).unwrap();
        assert_eq!(result.album_posts, c["count"].as_u64().unwrap() as usize);
        // 唯一样式差异：长联系人名称可换行，其余 HTML 仍逐字对照旧版。
        let expected = c["html"]
            .as_str()
            .unwrap()
            .replace("</style>", "h1{overflow-wrap:anywhere}\n</style>");
        assert_eq!(result.html, expected);
    }
}

#[test]
fn unsafe_paths_are_invisible() {
    for path in [
        "https://evil/a.png",
        "//evil/a",
        "../images/a",
        "images/../a",
        "images/./a",
        "images/a/b",
        "images\\a",
        "images/C:a",
        "images/",
        "images/..",
        "images/a\0.png",
        "images/a\n.png",
        "images/a\"onerror=x",
        "images/CON.png",
        "images/LPT1.txt",
        "images/a.",
        "images/a ",
        "videos/a.mp4",
        "file:///a",
        "data:image/png,x",
    ] {
        let result = render("x", &[json!({"media":[{"type":2,"local_file":path}]})]).unwrap();
        assert_eq!(result.album_posts, 0, "{path:?}");
        assert!(!result.html.contains("<img "));
    }
    for path in [
        "videos/../a",
        "https://evil/video",
        "images/a.mp4",
        "videos/a\\b",
    ] {
        assert_eq!(
            render("x", &[json!({"media":[{"type":6,"local_file":path}]})])
                .unwrap()
                .album_posts,
            0
        );
    }
}

#[test]
fn filenames_encoded_once_and_content_escaped() {
    let result = render("<script>", &[json!({"time":"<img>","content":"<script>\"'&[未知][微笑]","media":[{"type":2,"local_file":"images/中文%23#?.png"},{"type":6,"local_file":"videos/a%2f..%2fb.mp4"}]})]).unwrap();
    assert!(result
        .html
        .contains("images/%E4%B8%AD%E6%96%87%2523%23%3F.png"));
    assert!(result.html.contains("videos/a%252f..%252fb.mp4"));
    assert!(!result.html.contains("<script>"));
    assert!(result
        .html
        .contains("&lt;script&gt;&quot;&#x27;&amp;[未知]<span class=\"emoji\">🙂</span>"));
}

#[test]
fn dimensions_and_media_types() {
    for (width, height, expected) in [
        (json!(120), json!(80), true),
        (json!("120"), json!("80"), true),
        (json!(12.9), json!(true), true),
        (json!(0), json!(2), false),
        (json!(-1), json!(2), false),
        (json!("x\"onerror=x"), json!(2), false),
        (json!(null), json!(2), false),
        (json!([]), json!(2), false),
        (json!("12.3"), json!(2), false),
    ] {
        let html = render("x", &[json!({"media":[{"type":"2","local_file":"images/a.png","width":width,"height":height}]})]).unwrap().html;
        assert_eq!(html.contains(" width="), expected);
    }
    for kind in [
        json!(2.0),
        json!(true),
        json!(null),
        json!("02"),
        json!("6.0"),
    ] {
        assert_eq!(
            render(
                "x",
                &[json!({"media":[{"type":kind,"local_file":"images/a.png"}]})]
            )
            .unwrap()
            .album_posts,
            0
        );
    }
}

#[test]
fn malformed_media_and_empty_timeline() {
    for m in [
        json!(null),
        json!(false),
        json!({}),
        json!([null,false,{}, {"type":2,"local_file":true}]),
    ] {
        assert_eq!(render("x", &[json!({"media":m})]).unwrap().album_posts, 0);
    }
    let result = render("x", &[]).unwrap();
    assert_eq!(result.album_posts, 0);
    assert!(!result.html.contains("<nav"));
    assert!(result.html.ends_with("<main>\n</main></body></html>"));
}

#[test]
fn preview_is_current() {
    let posts: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/sns-album-render/preview.json"
    ))
    .unwrap();
    let result = render("合成验收", &posts).unwrap();
    assert_eq!(result.album_posts, 4);
    // 默认回归只比较预期 HTML，不覆盖样例；生产 render 不接触文件系统。
    let expected = std::fs::read_to_string(fixtures().join("preview.html")).unwrap();
    assert_eq!(
        result.html.replace("\r\n", "\n"),
        expected.replace("\r\n", "\n")
    );
}
