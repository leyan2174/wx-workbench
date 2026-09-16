fn main() {
    assert!(
        std::env::var("TARGET").as_deref() == Ok("x86_64-pc-windows-msvc"),
        "wx-workbench supports only Windows x64 (x86_64-pc-windows-msvc)"
    );
    // Clap 未优化的命令树超过 MSVC 默认的 1 MiB 主线程栈。
    println!("cargo:rustc-link-arg-bin=wx=/STACK:8388608");
}
