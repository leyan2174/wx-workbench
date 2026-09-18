//! 原生 SNS 解析、媒体和导出核心；调用者负责固定账号及显式输入输出边界。
//! 不自动发现配置或密钥；缓存和联网策略均由宿主明确传入。
pub(crate) mod album;
pub(crate) mod album_images;
pub(crate) mod album_render;
pub(crate) mod album_videos;
pub mod cache;
pub(crate) mod download;
mod export;

use crate::adapters::wechat::moments::decode;
#[cfg(test)]
use crate::adapters::wechat::moments::decode::{decode_content, Content};
#[cfg(test)]
use crate::adapters::wechat::moments::legacy::parse_timeline;
pub use crate::adapters::wechat::moments::legacy::TimeZone;
use crate::adapters::wechat::moments::legacy::{timestamp_filename, Post};
#[cfg(test)]
pub(crate) use export::export_database_with_media;
pub(crate) use export::{
    export_database_verified, export_database_with_publication, DownloadOptions,
    TimelinePublication, VerifiedPublication,
};
pub use export::{CacheRecovery, ExportOptions};

pub(crate) mod archive;
#[cfg(test)]
mod tests;
