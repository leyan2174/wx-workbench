// 直接编译生产源码，不注册公共模块，不替换 decoder 或 resolver。
#[path = "../../../src/attachment/attachment_id.rs"]
pub mod attachment_id;
pub use attachment_id::{AttachmentId, AttachmentKind};
#[path = "../../../src/attachment/decoder/mod.rs"]
pub mod decoder;
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
#[path = "../../../src/attachment/native_image.rs"]
pub mod native_image;
#[path = "../../../src/attachment/image_metadata.rs"]
pub mod image_metadata;
#[path = "../../../src/attachment/resolver.rs"]
pub mod resolver;
#[path = "../../support/resource_media_adapters.rs"]
pub mod adapters;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/toolkit/files.rs"]
pub mod files;
#[path = "../../../src/toolkit/setup.rs"]
pub mod setup;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
pub mod toolkit {
    pub use super::{files, private_file, setup};
    pub(crate) use super::files::ExportTarget;
}
pub mod attachment {
    pub(crate) use super::local_files;
}
