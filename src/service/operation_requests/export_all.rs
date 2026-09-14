use super::plan::Mode;
use anyhow::{ensure, Result};
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Args {
    /// 输出目录；默认选中配置旁 exported_chats
    pub output_dir: Option<PathBuf>,
    /// 导出时按数据库身份关联并转录语音
    pub with_transcriptions: bool,
    /// 生成计划 CSV，不导出聊天
    pub write_plan_csv: Option<PathBuf>,
    /// 读取计划 CSV，按 username 选择聊天
    pub from_plan_csv: Option<PathBuf>,
    #[serde(with = "crate::service::operations::plan_mode")]
    pub plan_mode: Mode,
    pub size_mode: super::chat_plan::Mode,
    /// 保留旧消息并追加本轮新消息
    pub incremental: bool,
    /// 只写新的 delta 批次，不读取或改写完整聊天文件
    pub delta_only: bool,
    pub start: Option<String>,
    pub end: Option<String>,
    pub dry_run: bool,
    pub users: Option<String>,
    pub asr: super::asr_batch::BatchArgs,
}

impl Args {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.write_plan_csv.is_none() || self.from_plan_csv.is_none(),
            "--write-plan-csv 与 --from-plan-csv 不能同时使用"
        );
        ensure!(
            !self.delta_only || self.start.is_some(),
            "--delta-only 需要 --start"
        );
        let start = self
            .start
            .as_deref()
            .map(crate::service::time::parse_timestamp)
            .transpose()?;
        let end = self
            .end
            .as_deref()
            .map(crate::service::time::parse_timestamp)
            .transpose()?;
        ensure!(
            !matches!((start, end), (Some(a), Some(b)) if a > b),
            "起始时间不能晚于结束时间"
        );
        Ok(())
    }
}
