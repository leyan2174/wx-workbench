use std::{env, fs, io::Write, path::Path};

fn main() {
    let args: Vec<_> = env::args().collect();
    let arg = |key: &str| args[args.iter().position(|value| value == key).unwrap() + 1].as_str();
    assert!(args.iter().any(|value| value == "--no-fallback"));
    assert!(args.iter().any(|value| value == "-oj"));
    assert_eq!(arg("-l"), "zh");
    assert_eq!(arg("-t"), "2");
    let model = Path::new(arg("-m"));
    let config = fs::read_to_string(model).unwrap();
    let (expected, text) = config.split_once('\n').unwrap();
    let audio = fs::read(arg("-f")).unwrap();
    assert!(audio.starts_with(b"RIFF"));
    assert_eq!(&audio[8..12], b"WAVE");
    assert_eq!(audio, fs::read(expected).unwrap());
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(model.with_extension("calls"))
            .unwrap(),
        "wav-verified"
    )
    .unwrap();
    if text == "FAIL" {
        std::process::exit(7);
    }
    assert!(!text.contains(['"', '\\', '\r', '\n']));
    fs::write(
        Path::new(arg("-of")).with_extension("json"),
        format!(
            "{{\"result\":{{\"language\":\"zh\"}},\"transcription\":[{{\"text\":\"{text}\"}}]}}"
        ),
    )
    .unwrap();
}
