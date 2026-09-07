use anyhow::Result;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Debug)]
pub struct Rendered {
    pub html: String,
    pub album_posts: usize,
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// JSON 标量遵循旧版 str(value or '')，容器保留 Python repr 的引号与布尔拼写。
fn py_string(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Value::String(k.clone())), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Number(n) => {
            let raw = n.to_string();
            if !raw.contains(['.', 'e', 'E']) {
                return raw;
            }
            let Some(f) = n.as_f64() else {
                return raw;
            };
            if f.is_infinite() {
                return if f.is_sign_positive() { "inf" } else { "-inf" }.into();
            }
            let text = format!("{f:?}");
            if let Some((mantissa, exponent)) = text.split_once('e') {
                let exponent: i32 = exponent.parse().unwrap();
                format!("{mantissa}e{exponent:+03}")
            } else {
                text
            }
        }
    }
}

fn py_repr(v: &Value) -> String {
    if let Value::String(s) = v {
        let quote = if s.contains('\'') && !s.contains('"') {
            '"'
        } else {
            '\''
        };
        let mut out = String::from(quote);
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c == quote => {
                    out.push('\\');
                    out.push(c);
                }
                c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push(quote);
        out
    } else {
        py_string(v)
    }
}

fn or_empty(v: &Value) -> String {
    if truthy(v) {
        py_string(v)
    } else {
        String::new()
    }
}

fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn clean_text(v: &Value) -> String {
    let normalized = or_empty(v).replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut newlines = 0;
    for c in normalized.chars() {
        if c <= '\u{1f}' && c != '\t' && c != '\n' {
            continue;
        }
        if c == '\n' {
            newlines += 1;
        } else {
            newlines = 0;
        }
        if newlines <= 2 {
            out.push(c);
        }
    }
    out.trim_matches(py_space).into()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn render_text(v: &Value) -> String {
    static PATTERN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[([^\[\]\s]{1,8})\]").unwrap());
    PATTERN
        .replace_all(
            &escape(&clean_text(v)),
            |caps: &regex::Captures<'_>| match emoji(&caps[1]) {
                Some(e) => format!("<span class=\"emoji\">{e}</span>"),
                None => caps[0].to_owned(),
            },
        )
        .replace('\n', "<br>")
}

fn emoji(s: &str) -> Option<&'static str> {
    Some(match s {
        "微笑" => "🙂",
        "笑脸" | "愉快" => "😄",
        "嘿哈" => "😆",
        "撇嘴" => "😒",
        "色" => "😍",
        "发呆" => "😳",
        "得意" => "😎",
        "流泪" => "😢",
        "害羞" => "😊",
        "闭嘴" => "🤐",
        "睡" => "😴",
        "大哭" => "😭",
        "尴尬" => "😅",
        "发怒" => "😡",
        "调皮" => "😜",
        "呲牙" => "😁",
        "惊讶" => "😮",
        "难过" => "😔",
        "酷" => "😎",
        "冷汗" => "😓",
        "抓狂" => "😫",
        "偷笑" => "🤭",
        "白眼" => "🙄",
        "悠闲" => "😌",
        "奋斗" => "💪",
        "疑问" => "❓",
        "嘘" => "🤫",
        "晕" => "😵",
        "囧" => "😳",
        "再见" => "👋",
        "擦汗" => "😅",
        "抠鼻" => "🙄",
        "鼓掌" => "👏",
        "坏笑" => "😏",
        "哈欠" => "🥱",
        "鄙视" => "😒",
        "委屈" => "🥺",
        "亲亲" => "😘",
        "可怜" => "🥺",
        "西瓜" => "🍉",
        "啤酒" => "🍺",
        "咖啡" => "☕",
        "饭" => "🍚",
        "玫瑰" => "🌹",
        "凋谢" => "🥀",
        "嘴唇" => "💋",
        "爱心" => "❤️",
        "心碎" => "💔",
        "蛋糕" => "🎂",
        "闪电" => "⚡",
        "炸弹" => "💣",
        "月亮" => "🌙",
        "太阳" => "☀️",
        "礼物" => "🎁",
        "拥抱" => "🤗",
        "强" => "👍",
        "弱" => "👎",
        "握手" => "🤝",
        "胜利" | "耶" => "✌️",
        "抱拳" | "合十" => "🙏",
        "拳头" => "✊",
        "爱你" => "🤟",
        "NO" => "🙅",
        "OK" => "👌",
        "庆祝" => "🎉",
        "捂脸" => "🤦",
        "烟花" => "🎆",
        "爆竹" => "🧨",
        _ => return None,
    })
}

pub fn safe_stem(value: &Value, fallback: &str) -> String {
    let cleaned: String = or_empty(value)
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let stem = cleaned.trim_matches(py_space).trim_end_matches(['.', ' ']);
    if stem.is_empty() {
        fallback.into()
    } else {
        stem.into()
    }
}

// 只生成一层本地媒体 URL；百分号也编码，避免文件名被二次解释为路径或查询。
fn local_src(media: &Value, directory: &str) -> Option<String> {
    let path = media.get("local_file")?.as_str()?;
    let filename = path.strip_prefix(directory)?.strip_prefix('/')?;
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.ends_with(['.', ' '])
        || filename
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|*".contains(c))
    {
        return None;
    }
    let base = filename.split('.').next()?.to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
    {
        return None;
    }
    let mut encoded = format!("{directory}/");
    for b in filename.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    Some(encoded)
}

fn media<'a>(post: &'a Value, video: bool) -> Vec<(&'a Value, String)> {
    post.get("media")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let kind = py_string(&m["type"]);
            let wanted = if video {
                kind == "6" || kind == "15"
            } else {
                kind == "2"
            };
            wanted
                .then(|| local_src(m, if video { "videos" } else { "images" }))
                .flatten()
                .map(|src| (m, src))
        })
        .collect()
}

fn dimension(v: &Value) -> Option<String> {
    let text = match v {
        Value::Bool(b) => u8::from(*b).to_string(),
        Value::Number(n) => {
            let raw = n.to_string();
            if raw.contains(['.', 'e', 'E']) {
                let f = n.as_f64()?;
                if !f.is_finite() || f <= 0.0 {
                    return None;
                }
                format!("{:.0}", f.trunc())
            } else {
                raw
            }
        }
        Value::String(s) => {
            static INTEGER: LazyLock<regex::Regex> =
                LazyLock::new(|| regex::Regex::new(r"^\+?\p{Nd}(?:_?\p{Nd})*$").unwrap());
            static DECIMAL: LazyLock<regex::Regex> =
                LazyLock::new(|| regex::Regex::new(r"^\p{Nd}$").unwrap());
            let s = s.trim_matches(py_space);
            if !INTEGER.is_match(s) {
                return None;
            }
            s.chars()
                .filter(|c| *c != '+' && *c != '_')
                .map(|c| {
                    // Unicode 十进制数字按十个连续编码排列；数学字体可能连续多组。
                    let mut first = c as u32;
                    while first > 0
                        && char::from_u32(first - 1)
                            .is_some_and(|p| DECIMAL.is_match(&p.to_string()))
                    {
                        first -= 1;
                    }
                    char::from(b'0' + ((c as u32 - first) % 10) as u8)
                })
                .collect()
        }
        _ => return None,
    };
    let text = text.trim_start_matches('0');
    (!text.is_empty() && !text.starts_with('-')).then(|| text.to_owned())
}

pub fn render(user: &str, posts: &[Value]) -> Result<Rendered> {
    let visible: Vec<_> = posts
        .iter()
        .filter(|p| {
            !clean_text(&p["content"]).is_empty()
                || !media(p, false).is_empty()
                || !media(p, true).is_empty()
        })
        .collect();
    let year_of = |p: &Value| or_empty(&p["time"]).chars().take(4).collect::<String>();
    let valid_year = |s: &str| {
        // Python isdigit 包含十进制数字及上标数字，但不包含罗马数字等 numeric 字符。
        static DIGITS: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(concat!(
                r"^[\p{Nd}\x{b2}\x{b3}\x{b9}\x{1369}-\x{1371}\x{19da}",
                r"\x{2070}\x{2074}-\x{2079}\x{2080}-\x{2089}\x{2460}-\x{2468}",
                r"\x{2474}-\x{247c}\x{2488}-\x{2490}\x{24ea}\x{24f5}-\x{24fd}\x{24ff}",
                r"\x{2776}-\x{277e}\x{2780}-\x{2788}\x{278a}-\x{2792}\x{10a40}-\x{10a43}",
                r"\x{10e60}-\x{10e68}\x{11052}-\x{1105a}\x{1f100}-\x{1f10a}]+$"
            ))
            .unwrap()
        });
        DIGITS.is_match(s)
    };
    let mut years = Vec::new();
    for p in &visible {
        let y = year_of(p);
        if valid_year(&y) && !years.contains(&y) {
            years.push(y);
        }
    }
    let user = escape(user);
    let mut parts: Vec<String> = vec![
        "<!doctype html>".into(),
        "<html lang=\"zh-CN\"><head><meta charset=\"utf-8\">".into(),
        "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\">".into(),
        format!("<title>{user}朋友圈</title>"),
        "<style>".into(),
        "*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:#fafafa;color:#202124;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI','Microsoft YaHei',sans-serif;letter-spacing:0;line-height:1.7}".into(),
        "header{max-width:780px;margin:0 auto;padding:42px 18px 20px}h1{margin:0;font-size:30px;font-weight:700}.year-nav{position:sticky;top:0;z-index:3;display:flex;gap:8px;overflow-x:auto;padding:10px max(14px,calc((100vw - 780px)/2 + 18px));background:rgba(250,250,250,.94);border-block:1px solid #e5e7eb;backdrop-filter:blur(12px)}".into(),
        ".year-nav a{flex:0 0 auto;color:#334155;text-decoration:none;padding:5px 10px;border-radius:6px;background:#eef2f7;font-size:14px}main{max-width:780px;margin:0 auto;padding:0 18px 52px}.year{scroll-margin-top:62px;margin:34px 0 8px;font-size:24px}.post{padding:20px 0;border-top:1px solid #e5e7eb}.time{color:#64748b;font-size:14px;margin-bottom:7px}.content{white-space:normal;font-size:16px;overflow-wrap:anywhere}.emoji{font-family:'Segoe UI Emoji','Apple Color Emoji',sans-serif}.imgs{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:7px;margin-top:13px}.imgs a{display:block;min-width:0}.imgs img{display:block;width:100%;height:auto;max-height:620px;object-fit:cover;background:#e5e7eb;border-radius:6px}.videos{display:grid;gap:10px;margin-top:13px}.videos video{display:block;width:100%;max-height:720px;background:#111;border-radius:6px}".into(),
        "@media(max-width:560px){header{padding:28px 14px 14px}h1{font-size:25px}main{padding:0 14px 40px}.year{font-size:21px}.post{padding:17px 0}.imgs{grid-template-columns:repeat(2,minmax(0,1fr))}.imgs:has(a:only-child){grid-template-columns:1fr}.imgs:has(a:only-child) img{object-fit:contain}}".into(),
        "h1{overflow-wrap:anywhere}".into(),
        "</style></head><body>".into(),
        format!("<header><h1>{user}朋友圈</h1></header>"),
    ];
    if !years.is_empty() {
        parts.push("<nav class=\"year-nav\" aria-label=\"年份\">".into());
        parts.extend(
            years
                .iter()
                .map(|y| format!("<a href=\"#year-{y}\">{y}</a>")),
        );
        parts.push("</nav>".into());
    }
    parts.push("<main>".into());
    let mut current_year = None;
    for p in &visible {
        let y = year_of(p);
        if valid_year(&y) && current_year.as_ref() != Some(&y) {
            parts.push(format!("<h2 class=\"year\" id=\"year-{y}\">{y}</h2>"));
            current_year = Some(y);
        }
        parts.push("<article class=\"post\">".into());
        parts.push(format!(
            "<div class=\"time\">{}</div>",
            escape(&or_empty(&p["time"]))
        ));
        let content = clean_text(&p["content"]);
        if !content.is_empty() {
            parts.push(format!(
                "<div class=\"content\">{}</div>",
                render_text(&Value::String(content))
            ));
        }
        for video in [false, true] {
            let items = media(p, video);
            if items.is_empty() {
                continue;
            }
            parts.push(format!(
                "<div class=\"{}\">",
                if video { "videos" } else { "imgs" }
            ));
            for (m, src) in items {
                if video {
                    parts.push(format!(
                        "<video src=\"{src}\" controls preload=\"metadata\" playsinline></video>"
                    ));
                } else {
                    let dimensions = dimension(&m["width"])
                        .zip(dimension(&m["height"]))
                        .map(|(w, h)| format!(" width=\"{w}\" height=\"{h}\""))
                        .unwrap_or_default();
                    parts.push(format!("<a href=\"{src}\" target=\"_blank\"><img src=\"{src}\"{dimensions} loading=\"lazy\" alt=\"\"></a>"));
                }
            }
            parts.push("</div>".into());
        }
        parts.push("</article>".into());
    }
    parts.push("</main></body></html>".into());
    Ok(Rendered {
        html: parts.join("\n"),
        album_posts: visible.len(),
    })
}

#[cfg(test)]
#[path = "album_render_tests.rs"]
mod tests;
