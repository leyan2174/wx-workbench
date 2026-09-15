// 直接引用生产模块；没有缓存、解密或映射替身。
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
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
pub mod runtime;
pub mod daemon {
    pub use crate::production_cache as cache;
}
#[path = "../../../src/adapters/wechat/emoticons/catalog.rs"]
pub mod catalog;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
#[path = "../../../src/adapters/wechat/emoticons/types.rs"]
pub mod types;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/key_store/mod.rs"]
pub mod key_store;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/toolkit/setup.rs"]
pub mod setup;
pub mod toolkit {
    pub use crate::{private_file, setup};
}
