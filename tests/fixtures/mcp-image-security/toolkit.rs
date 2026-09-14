#![allow(dead_code)]
pub mod asr {
    use anyhow::{ensure, Result};
    include!(concat!(env!("OUT_DIR"), "/wav_validator.rs"));
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
