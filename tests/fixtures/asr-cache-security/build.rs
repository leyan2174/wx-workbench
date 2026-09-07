// 被引用的原测试使用主仓库相对路径；只在本 harness 内准备同名合成夹具。
use std::{env, fs, path::PathBuf};

fn main() {
    let harness = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source_root = harness.join("..").canonicalize().unwrap();
    let target_root = harness.join("tests/fixtures");
    let mut files = vec!["asr-local/fake.rs".to_owned()];
    for stem in ["tone", "silence", "sweep", "multi40", "multi100"] {
        for extension in ["silk", "pcm"] {
            files.push(format!("audio/{stem}.{extension}"));
        }
    }
    for relative in files {
        let source = source_root.join(&relative);
        let target = target_root.join(&relative);
        println!("cargo:rerun-if-changed={}", source.display());
        println!("cargo:rerun-if-changed={}", target.display());
        let bytes = fs::read(&source).expect("read existing synthetic fixture");
        if fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, &bytes).expect("stage fixture inside owned harness");
        }
        assert_eq!(fs::read(&target).unwrap(), bytes);
    }
}
