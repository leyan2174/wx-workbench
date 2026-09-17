// 独立编译生产文件及真实守卫、WASM runtime。
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
#[path = "../../../src/application/moments/album_images.rs"]
mod album_images;
#[path = "../../../src/adapters/wechat/moments/decode.rs"]
mod decode;
#[path = "../../../src/adapters/wechat/media/sns_keystream.rs"]
pub(crate) mod keystream;
