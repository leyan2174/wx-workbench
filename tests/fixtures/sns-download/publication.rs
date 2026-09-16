//! Real runtime and single-file publisher dependencies for the SNS fixtures.
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/service/monitor.rs"]
pub mod monitor_contract;
pub mod service {
    pub mod protocol {
        pub use super::super::monitor_contract as monitor;
    }
}
#[path = "../../../src/daemon/cache.rs"]
#[allow(unused_imports)] // The download fixture does not consume the ResourceSnapshot re-export.
pub mod production_cache;
#[path = "../../../src/adapters/wechat/messages/probe.rs"]
pub mod message_probe;
pub mod adapters {
    pub mod wechat {
        pub mod messages {
            pub use super::super::super::message_probe as probe;
        }
    }
}
#[path = "../../../src/runtime.rs"]
pub mod runtime;
pub mod daemon {
    pub use super::production_cache as cache;
}
#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // Download fixture uses publication guards, not directory collection.
pub mod files;
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Store is embedded for publication protection, not migration orchestration.
pub mod key_store;
#[path = "../../../src/infrastructure/configuration.rs"]
pub mod setup;
pub mod toolkit {
    pub use super::files;
}
