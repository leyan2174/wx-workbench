#[path = "../../src/adapters/wechat/media/resource.rs"]
pub mod resource;
#[path = "../../src/adapters/wechat/media/legacy_dat.rs"]
pub mod legacy_dat;
#[path = "../../src/adapters/wechat/media/attachment_kind.rs"]
pub mod attachment_kind;
pub mod wechat {
    pub mod media {
        pub use super::super::resource;
        pub use super::super::legacy_dat;
        pub use super::super::attachment_kind;
    }
}
