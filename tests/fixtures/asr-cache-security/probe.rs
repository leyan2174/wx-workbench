// 只接受测试模型文件；模拟后端诊断与写入攻击，不包含任何模型或网络代码。
use std::{env, fs, path::PathBuf};
fn main() {
    let args: Vec<String> = env::args().collect();
    let arg = |name: &str| &args[args.iter().position(|a| a == name).unwrap() + 1];
    let model = PathBuf::from(arg("-m"));
    let mode = fs::read_to_string(&model).unwrap();
    if mode == "fail" {
        eprintln!("SYNTHETIC_SECRET_BACKEND_DIAGNOSTIC");
        std::process::exit(7);
    }
    if mode == "probe" && fs::OpenOptions::new().write(true).open(&model).is_ok() {
        std::process::exit(9);
    }
    fs::write(
        PathBuf::from(arg("-of")).with_extension("txt"),
        "synthetic ok",
    )
    .unwrap();
}
