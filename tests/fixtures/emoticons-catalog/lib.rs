// 直接引用生产模块；没有缓存、解密或映射替身。
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/service/monitor.rs"]
pub mod monitor_contract;
pub mod service {
    pub mod protocol {
        pub use crate::monitor_contract as monitor;
    }
}
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code, unused_imports)] // This slice omits snapshot re-exports and unrelated cache lifecycle entry points.
pub mod production_cache;
#[path = "../../../src/adapters/wechat/messages/probe.rs"]
pub mod message_probe;
pub mod adapters {
    pub mod wechat {
        pub mod messages {
            pub use crate::message_probe as probe;
        }
    }
}
#[path = "../../../src/runtime.rs"]
#[allow(dead_code)] // This fixed-account fixture omits bootstrap and other operation lifecycle entry points.
pub mod runtime;
pub mod daemon {
    pub use crate::production_cache as cache;
}
#[path = "../../../src/adapters/wechat/emoticons/catalog.rs"]
pub mod catalog;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub mod local_files;
#[path = "../../../src/adapters/wechat/emoticons/types.rs"]
pub mod types;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Catalog fixture does not exercise the store's legacy import/seed entry points.
pub mod key_store;
#[path = "../../../src/private_file.rs"]
pub mod private_file;
#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // Catalog fixture uses only the guards required by configuration snapshots.
pub mod files;
#[path = "../../../src/infrastructure/configuration.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
pub mod infrastructure {
    pub(crate) use crate::files as publication;
    pub(crate) use crate::setup as configuration;
}
