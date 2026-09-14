use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Args {
    /// 输出根目录；默认须全新，--append-run 可复用已有普通目录
    pub output: PathBuf,
    /// 精确 username，逗号分隔；默认 WECHAT_EXPORT_USERS 或全部会话
    pub users: Option<String>,
    /// 含端点的起始时间：本地日期、日期时间或 Unix 秒
    pub start: String,
    /// 含端点的结束时间；仅日期表示当天零点
    pub end: Option<String>,
    /// 批次目录名；默认本机时间及纳秒，不接受路径或设备名
    pub run_id: Option<String>,
    /// 在已有输出根目录中创建全新批次，不覆盖任何已有 run
    pub append_run: bool,
}
