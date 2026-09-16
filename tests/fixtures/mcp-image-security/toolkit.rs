#![allow(dead_code)]
pub use crate::files;
pub mod asr {
    pub use crate::audio_wav::validate_wav;
}
mod images {
    use anyhow::{ensure, Context, Result};
    include!(concat!(env!("OUT_DIR"), "/image_key_parsers.rs"));
}
pub fn parse_image_aes(value: &str) -> anyhow::Result<[u8; 16]> {
    images::parse_aes(value)
}
pub fn parse_image_xor(value: &str) -> anyhow::Result<u8> {
    images::parse_xor(value)
}
