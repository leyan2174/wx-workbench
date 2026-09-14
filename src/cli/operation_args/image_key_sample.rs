use std::path::PathBuf;

#[derive(Debug, Clone, Default, clap::Args)]
pub struct SampleArgs {
    /// 显式指定当前账号 msg/attach 目录内的 V2 DAT 文件。
    #[arg(long, requires = "sample_output")]
    pub sample_input: Option<PathBuf>,
    /// 显式指定新输出文件，其父目录必须已存在且位于受保护的账号数据之外。
    #[arg(long, requires = "sample_input")]
    pub sample_output: Option<PathBuf>,
}

impl From<SampleArgs> for crate::service::operation_requests::image_key_sample::SampleArgs {
    fn from(value: SampleArgs) -> Self {
        Self {
            sample_input: value.sample_input,
            sample_output: value.sample_output,
        }
    }
}

impl From<crate::service::operation_requests::image_key_sample::SampleArgs> for SampleArgs {
    fn from(value: crate::service::operation_requests::image_key_sample::SampleArgs) -> Self {
        Self {
            sample_input: value.sample_input,
            sample_output: value.sample_output,
        }
    }
}
