// 独立注册生产模块；不改 main 的模块树和 Cargo 清单。
#![allow(dead_code)]
#[path = "../../../src/private_file.rs"]
mod private_file;
#[path = "../sns-download/publication.rs"]
mod publication;
pub use publication::{config, crypto, daemon, key_store, runtime, toolkit};
mod adapters {
    pub mod wechat {
        pub use crate::publication::adapters::wechat::messages;
        pub mod media {
            pub(crate) use crate::keystream as sns_keystream;
        }
    }
}
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/application/moments/album_videos.rs"]
mod album_videos;
#[path = "../../../src/adapters/wechat/media/sns_keystream.rs"]
pub(crate) mod keystream;
