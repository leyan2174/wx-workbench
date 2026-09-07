// 合成后端模拟解码失败时回显输入内容；不读取任何环境凭证。
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let index = args.iter().position(|arg| arg == "-f").unwrap();
    let bytes = std::fs::read(&args[index + 1]).unwrap();
    eprintln!("decoder diagnostic: {}", String::from_utf8_lossy(&bytes));
    std::process::exit(9);
}
