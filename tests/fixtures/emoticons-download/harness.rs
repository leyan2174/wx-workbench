// Standalone test entry until the mainline connects toolkit::emoticons.
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment { pub(crate) use crate::local_files; }
#[path = "../../../src/toolkit/emoticons/types.rs"]
pub mod types;
#[path = "../../../src/toolkit/emoticons/download.rs"]
pub mod download;
