//! 原生 SNS 解析、媒体和导出核心；调用者负责固定账号及显式输入输出边界。
//! 不自动发现配置或密钥；缓存和联网策略均由宿主明确传入。
pub(crate) mod album;
pub(crate) mod album_images;
pub(crate) mod album_render;
pub(crate) mod album_videos;
pub mod cache;
mod decode;
pub(crate) mod download;
mod export;
mod parse;
pub(crate) mod publish;
pub mod video_runtime;

use decode::{decode_content, sanitize_xml, Content};
pub(crate) use export::{
    export_database_with_media, export_database_with_publication, DownloadOptions,
    TimelinePublication,
};
pub use export::{CacheRecovery, ExportOptions};
pub use parse::TimeZone;
use parse::{parse_timeline, timestamp_filename, Comment, Post};

pub(crate) mod archive;
#[cfg(test)]
mod tests;
