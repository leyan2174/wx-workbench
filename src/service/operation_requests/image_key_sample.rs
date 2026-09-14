use anyhow::{bail, ensure, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SampleArgs {
    /// 显式指定当前账号 msg/attach 目录内的 V2 DAT 文件。
    pub sample_input: Option<PathBuf>,
    /// 显式指定新输出文件，其父目录必须已存在且位于受保护的账号数据之外。
    pub sample_output: Option<PathBuf>,
}

impl SampleArgs {
    pub(crate) fn validate_request(&self) -> Result<()> {
        match (&self.sample_input, &self.sample_output) {
            (None, None) => Ok(()),
            (Some(input), Some(output)) => {
                ensure!(input.is_absolute(), "样本输入必须使用绝对路径");
                ensure!(output.is_absolute(), "样本输出必须使用绝对路径");
                ensure!(
                    input
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("dat")),
                    "样本输入必须是 DAT 文件"
                );
                ensure!(input != output, "样本输出不能覆盖输入");
                Ok(())
            }
            _ => bail!("--sample-input 与 --sample-output 必须同时提供"),
        }
    }
}
