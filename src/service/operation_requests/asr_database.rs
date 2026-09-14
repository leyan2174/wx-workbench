use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TranscribeDatabaseNativeArgs {
    /// 显式单账号静态已解密根目录；来源由调用方保证，并非已认证账号
    pub decrypted_dir: PathBuf,
    /// 精确 username，不按昵称或备注推断
    pub username: String,
    /// 完整消息分片来源，例如 message/message_0.db
    pub source: String,
    /// 此消息分片中该联系人消息表的 local_id，不是媒体库 local_id
    pub local_id: i64,
    /// 可选成功转录缓存；必须位于可信、稳定且独立于数据库的目录
    pub cache_file: Option<PathBuf>,
    /// 调用方显式提供的缓存账号命名空间；不是账号认证
    pub cache_account: Option<String>,
    pub backend: super::asr::BackendArgs,
}
