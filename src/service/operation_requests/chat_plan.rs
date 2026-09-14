use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Estimate,
    Scan,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 明确指定已解密缓存根目录；不读取配置或发现账号
    pub decrypted_dir: PathBuf,
    /// 相对解密根目录的消息数据库路径，可重复；不指定时保留缺库状态
    pub message_dbs: Vec<PathBuf>,
    /// 相对解密根目录的 message_resource 数据库路径
    pub resource_db: Option<PathBuf>,
    /// 相对解密根目录的语音数据库路径，可重复
    pub media_dbs: Vec<PathBuf>,
    /// 明确的 username，可重复；提供元数据清单时作为精确过滤
    pub users: Vec<String>,
    /// 聊天元数据 JSON 数组：username、index、chat_name/display_name、chat_type/kind
    pub chats_json: Option<PathBuf>,
    /// 精确排除 username，可重复
    pub exclude_users: Vec<String>,
    /// 统计模式；scan 另需显式源目录或媒体目录
    pub size_mode: Mode,
    /// 账号源目录，扫描其 msg 子目录；与 media-dir 二选一
    pub source_dir: Option<PathBuf>,
    /// 直接指定包含 attach/file/video 的媒体目录
    pub media_dir: Option<PathBuf>,
    /// 扫描线程数，结果仍按清单顺序输出
    pub threads: u8,
    /// 含端点的起始本地时间：日期、日期时间或 Unix 秒
    pub start: Option<String>,
    /// 含端点的结束时间；仅日期表示当天零点
    pub end: Option<String>,
    /// 新 CSV 文件；父目录须存在，禁止覆盖或写入源目录
    pub output: PathBuf,
}
