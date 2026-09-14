use std::path::PathBuf;

#[derive(clap::Args, Clone, Debug)]
pub struct TranscribeDatabaseNativeArgs {
    /// 显式单账号静态已解密根目录；来源由调用方保证，并非已认证账号
    #[arg(long)]
    pub decrypted_dir: PathBuf,
    /// 精确 username，不按昵称或备注推断
    #[arg(long)]
    pub username: String,
    /// 完整消息分片来源，例如 message/message_0.db
    #[arg(long)]
    pub source: String,
    /// 此消息分片中该联系人消息表的 local_id，不是媒体库 local_id
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    pub local_id: i64,
    /// 可选成功转录缓存；必须位于可信、稳定且独立于数据库的目录
    #[arg(long, requires = "cache_account")]
    pub cache_file: Option<PathBuf>,
    /// 调用方显式提供的缓存账号命名空间；不是账号认证
    #[arg(long, requires = "cache_file")]
    pub cache_account: Option<String>,
    #[command(flatten)]
    pub backend: super::asr::BackendArgs,
}

impl From<TranscribeDatabaseNativeArgs>
    for crate::service::operation_requests::asr_database::TranscribeDatabaseNativeArgs
{
    fn from(value: TranscribeDatabaseNativeArgs) -> Self {
        Self {
            decrypted_dir: value.decrypted_dir,
            username: value.username,
            source: value.source,
            local_id: value.local_id,
            cache_file: value.cache_file,
            cache_account: value.cache_account,
            backend: value.backend.into(),
        }
    }
}

impl From<crate::service::operation_requests::asr_database::TranscribeDatabaseNativeArgs>
    for TranscribeDatabaseNativeArgs
{
    fn from(
        value: crate::service::operation_requests::asr_database::TranscribeDatabaseNativeArgs,
    ) -> Self {
        Self {
            decrypted_dir: value.decrypted_dir,
            username: value.username,
            source: value.source,
            local_id: value.local_id,
            cache_file: value.cache_file,
            cache_account: value.cache_account,
            backend: value.backend.into(),
        }
    }
}
