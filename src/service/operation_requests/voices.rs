/// CLI 解析和后台执行共用字段定义；IPC 仍保留原来的扁平字段。
pub struct Args {
    /// 会话名称（可选；省略则导出全部语音）
    pub chat: Option<String>,
    /// 输出目录
    pub output: String,
    /// 最多导出条数
    pub limit: Option<usize>,
    /// 分页偏移
    pub offset: usize,
    /// 起始时间 YYYY-MM-DD
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    pub until: Option<String>,
    /// 目标已存在时覆盖
    pub overwrite: bool,
    /// 输出 JSON（默认 YAML）
    pub json: bool,
}
