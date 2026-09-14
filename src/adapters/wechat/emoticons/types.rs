//! 表情映射的内部数据；URL 与解密材料不直接投影到 CLI 输出。

#[derive(Clone, Default)]
pub struct EmojiInfo {
    pub cdn_url: String,
    pub aes_key: String,
    pub encrypt_url: String,
    pub product_id: String,
    pub caption: Option<String>,
}

#[derive(Clone)]
pub struct Emoji {
    pub md5: String,
    pub info: EmojiInfo,
}

pub struct Catalog {
    /// 保留旧映射的首次插入顺序；重复 MD5 更新值，但不移动原位置。
    pub items: Vec<Emoji>,
    pub non_store_count: usize,
    pub store_added: usize,
    pub source_available: bool,
}
