#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 明确授权只读扫描配置中微信进程的内存
    pub authorize_memory_scan: bool,
}
