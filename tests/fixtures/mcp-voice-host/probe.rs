use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    let args: Vec<_> = env::args().collect();
    let arg = |key: &str| &args[args.iter().position(|s| s == key).unwrap() + 1];
    let model = PathBuf::from(arg("-m"));
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(model.with_extension("calls"))
        .unwrap();
    calls.write_all(b"called\n").unwrap();
    if fs::read(&model).unwrap() == b"sleep" {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    assert!(fs::read(arg("-f")).unwrap().starts_with(b"RIFF"));
    fs::write(model.with_extension("wav-path"), arg("-f")).unwrap();
    fs::write(
        PathBuf::from(arg("-of")).with_extension("json"),
        r#"{"text":"synthetic transcript","language":"zh"}"#,
    )
    .unwrap();
}
