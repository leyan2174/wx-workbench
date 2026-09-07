//! 小型消息 XML 的统一边界；大合并记录另有显式大小契约。
use roxmltree::Document;

pub(crate) fn parse(content: &str) -> Option<Document<'_>> {
    // 与旧解析器一致，按字符数限制输入，并拒绝 DTD/实体声明。
    if content.is_empty() || content.chars().take(20_001).count() > 20_000 {
        return None;
    }
    let upper = content.to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return None;
    }
    Document::parse(content).ok()
}

pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
