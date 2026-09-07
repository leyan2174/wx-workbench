// 未接线阶段直接编译生产文件及真实守卫、WASM runtime。
#![allow(dead_code)]
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/toolkit/sns/album_images.rs"]
mod album_images;
#[path = "../../../src/toolkit/sns/video_runtime.rs"]
mod video_runtime;
