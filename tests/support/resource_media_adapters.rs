#[path = "../../src/adapters/wechat/media/resource.rs"]
pub mod resource;
pub mod wechat {
    pub mod media {
        pub use super::super::resource;
    }
}
