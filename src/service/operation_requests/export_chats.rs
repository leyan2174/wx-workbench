use super::plan::Mode;
use std::path::PathBuf;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 输出目录
    pub output_dir: PathBuf,
    /// 仅导出指定 username，逗号分隔；也可设 WECHAT_EXPORT_USERS
    pub users: Option<String>,
    /// 保留旧消息并按分片来源和 local_id 追加；身份歧义时拒绝覆盖
    pub incremental: bool,
    /// 起始本地时间（含端点）：日期、日期时间或 Unix 秒
    pub start: Option<String>,
    /// 结束本地时间（含端点）；仅日期表示当天零点
    pub end: Option<String>,
    /// 只列出会话，不创建输出目录或索引
    pub dry_run: bool,
    /// 读取计划 CSV，以 username 精确选择会话
    pub from_plan_csv: Option<PathBuf>,
    /// blacklist 仅跳过 export=0（默认）；whitelist 仅选择 export=1
    #[serde(with = "crate::service::operations::optional_plan_mode")]
    pub plan_mode: Option<Mode>,
}
