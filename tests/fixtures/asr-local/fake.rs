use std::{env, fs, path::PathBuf, thread, time::Duration};
fn main() {
    let args: Vec<String> = env::args().collect();
    let arg = |key: &str| -> &str { &args[args.iter().position(|s| s == key).unwrap() + 1] };
    assert!(args.iter().any(|s| s == "--no-fallback"));
    assert_eq!(arg("-t"), "2");
    assert_eq!(arg("-l"), "zh");
    assert!(PathBuf::from(arg("-f")).is_file());
    let mode = fs::read_to_string(arg("-m")).unwrap();
    if mode == "sleep" {
        thread::sleep(Duration::from_secs(30));
    }
    if mode == "fail" {
        eprintln!("synthetic failure");
        std::process::exit(7);
    }
    if mode == "missing" {
        return;
    }
    let json = args.iter().any(|s| s == "-oj");
    let content = if mode == "malformed" {
        "{broken"
    } else if json {
        r#"{"result":{"language":"zh"},"transcription":[{"text":"hello"},{"text":" world"}]}"#
    } else {
        " hello world \n"
    };
    fs::write(
        PathBuf::from(arg("-of")).with_extension(if json { "json" } else { "txt" }),
        content,
    )
    .unwrap();
}
