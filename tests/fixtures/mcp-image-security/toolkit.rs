#![allow(dead_code)]
use std::path::{Path, PathBuf};
pub mod asr {
    use anyhow::{ensure, Result};
    include!(concat!(env!("OUT_DIR"), "/wav_validator.rs"));
}
#[path = "../../../src/toolkit/images.rs"]
mod images;
pub fn parse_image_aes(value: &str) -> anyhow::Result<[u8; 16]> {
    images::parse_aes(value)
}
pub fn parse_image_xor(value: &str) -> anyhow::Result<u8> {
    images::parse_xor(value)
}
fn raw_config() -> anyhow::Result<(PathBuf, serde_json::Value)> {
    panic!("forbidden automatic image config lookup")
}
#[derive(Default)]
struct Report {
    total: usize,
    written: usize,
    skipped: usize,
    failures: Vec<Failure>,
}
struct Failure {
    path: PathBuf,
    error: String,
}
impl Report {
    fn finish(&self) -> anyhow::Result<()> {
        panic!("unrelated batch entry")
    }
}
mod files {
    use super::*;
    pub fn resolved(_: &Path) -> anyhow::Result<PathBuf> {
        panic!("unrelated batch entry")
    }
    pub fn separate(_: &Path, _: &Path) -> anyhow::Result<()> {
        panic!("unrelated batch entry")
    }
    pub fn collect(_: &Path, _: &str, _: bool) -> anyhow::Result<Vec<PathBuf>> {
        panic!("unrelated batch entry")
    }
    pub fn atomic_output(
        _: &Path,
        _: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        panic!("unrelated batch entry")
    }
}
