use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    let args: Vec<_> = env::args().collect();
    let arg = |key: &str| &args[args.iter().position(|s| s == key).unwrap() + 1];
    let model = PathBuf::from(arg("-m"));
    let spec: serde_json::Value = serde_json::from_slice(&fs::read(&model).unwrap()).unwrap();
    let wav = PathBuf::from(arg("-f"));
    assert_eq!(
        fs::read(&wav).unwrap(),
        mcp_voice_host_security::infrastructure::audio::prepare_wav_bytes(include_bytes!(
            "../audio/silence.silk"
        ))
        .unwrap()
    );
    let mut log = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(model.with_extension("calls"))
        .unwrap();
    writeln!(log, "{}", wav.display()).unwrap();
    match spec["mode"].as_str().unwrap_or("ok") {
        "fail" => {
            eprintln!("SYNTHETIC_BACKEND_SECRET");
            std::process::exit(7);
        }
        "sleep" => std::thread::sleep(std::time::Duration::from_secs(5)),
        "malformed" => {
            fs::write(
                PathBuf::from(arg("-of")).with_extension("json"),
                b"SYNTHETIC_INVALID_JSON",
            )
            .unwrap();
            return;
        }
        _ => {}
    }
    fs::write(PathBuf::from(arg("-of")).with_extension("json"), serde_json::to_vec(&serde_json::json!({"text":spec["text"].as_str().unwrap_or("synthetic transcript"),"language":"zh"})).unwrap()).unwrap();
}
