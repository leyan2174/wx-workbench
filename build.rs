fn main() {
    assert!(
        std::env::var("TARGET").as_deref() == Ok("x86_64-pc-windows-msvc"),
        "wx-cli supports only Windows x64 (x86_64-pc-windows-msvc)"
    );
}
