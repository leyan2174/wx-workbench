// 直接编译生产源码，不注册公共模块，不替换 decoder 或 resolver。
#[path = "../../../src/attachment/attachment_id.rs"]
pub mod attachment_id;
pub use attachment_id::{AttachmentId, AttachmentKind};
#[path = "../../../src/attachment/decoder/mod.rs"]
pub mod decoder;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub(crate) mod local_files;
#[path = "../../../src/attachment/native_image.rs"]
#[allow(dead_code)] // Resolver/decoder tests omit some publication entry points.
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
#[allow(dead_code)] // This fixed-account fixture omits bootstrap and other operation lifecycle entry points.
pub mod runtime;
#[path = "../../../src/toolkit/files.rs"]
#[allow(dead_code)] // Image fixture uses publication guards, not directory collection.
pub mod files;
#[path = "../../../src/toolkit/setup.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
pub mod toolkit {
    pub use super::{files, private_file, setup};
    pub(crate) use super::files::ExportTarget;
}
pub mod attachment {
    pub use super::AttachmentKind;
    pub(crate) use super::local_files;
}
