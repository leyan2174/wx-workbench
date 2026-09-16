//! 独立 ASR 测试壳共用真实账号上下文和缓存模块，不替换解密或发布行为。
#[path = "../../src/config.rs"]
pub mod config;
#[path = "../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../src/runtime.rs"]
pub mod runtime;
#[path = "../../src/service/monitor.rs"]
pub mod monitor_contract;
#[path = "../../src/daemon/cache.rs"]
#[allow(unused_imports)] // ASR 测试不调用图片资源快照接口。
pub mod db_cache;

pub mod daemon {
    pub use super::db_cache as cache;
}
