// 独立注册生产模块；不改 main 的模块树和 Cargo 清单。
#![allow(dead_code)]
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/toolkit/sns/album_videos.rs"]
mod album_videos;
#[path = "../../../src/toolkit/sns/video_runtime.rs"]
mod video_runtime;
