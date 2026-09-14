"""临时注册新模块，编译真实 DbCache、正文模块及父查询所需定义。"""
import os
from pathlib import Path
import re
import runpy
import shutil
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
execute = runpy.run_path(str(HERE / "generate_oracle.py"))["execute"]


def module(name, relative):
    return f'#[path = "{(REPO / relative).as_posix()}"] pub mod {name};\n'


def main():
    with tempfile.TemporaryDirectory(prefix="wx-delta-query-rust-") as directory:
        root = Path(directory)
        (root / "tests/fixtures").mkdir(parents=True)
        shutil.copy2(REPO / "tests/fixtures/transfer-golden.json", root / "tests/fixtures/transfer-golden.json")
        source = (REPO / "src/daemon/query.rs").read_text(encoding="utf-8")
        # 原样选取已核实的顶层声明，不复制或替代其内部实现。
        declarations = []
        for header in ["pub struct Names", "impl Names", "fn msg_table_re", "fn current_unknown_shards", "fn resolve_username"]:
            found = re.search(r"(?ms)^" + re.escape(header) + r"\b[^\n]*\n.*?^}", source)
            if not found:
                raise RuntimeError(f"parent declaration changed: {header}")
            declarations.append(found.group(0))
        query = "use std::collections::HashMap;\nuse std::sync::OnceLock;\nuse regex::Regex;\nuse super::cache::DbCache;\nuse super::meta::discover_unknown_shards;\n"
        query += "\n".join(declarations) + "\n" + module("export_delta", "src/daemon/query/export_delta.rs")
        (root / "query.rs").write_text(query, encoding="utf-8")
        lib = "".join(module(name, path) for name, path in [
            ("config", "src/config.rs"), ("runtime", "src/runtime.rs"),
            ("crypto", "src/crypto/mod.rs"), ("message", "src/message/mod.rs")])
        lib += "pub mod toolkit {\n" + module("chat_delta", "src/toolkit/chat_delta.rs") + module("contact_metadata", "src/toolkit/contact_metadata.rs") + "}\n"
        lib += "pub mod daemon {\n" + module("cache", "src/daemon/cache.rs") + module("meta", "src/daemon/meta.rs")
        lib += f'#[path = "{(root / "query.rs").as_posix()}"] pub mod query;\n' + "}\n"
        lib += "pub mod cli {\n" + module("export_delta", "src/cli/export_delta.rs") + "}\n"
        (root / "lib.rs").write_text(lib, encoding="utf-8")
        (root / "Cargo.toml").write_text('''[package]
name = "delta-query-harness"
version = "0.0.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "=1.0.140", features = ["arbitrary_precision"] }
sha2 = "0.10"
tempfile = "3"
rusqlite = { version = "0.31", features = ["bundled"] }
tokio = { version = "1", features = ["full"] }
md5 = "0.7"
regex = "1"
zstd = "0.13"
roxmltree = "0.20"
base64 = "0.22"
dirs = "5"
aes = "0.8"
cbc = { version = "0.1", features = ["alloc"] }
hmac = "0.12"
pbkdf2 = "0.12"
zeroize = "1"
windows = { version = "0.58", features = ["Win32_Foundation", "Win32_System_Com", "Win32_UI_Shell"] }
''', encoding="utf-8")
        env = os.environ.copy()
        for command in ("check", "test"):
            args = ["cargo", command, "--manifest-path", str(root / "Cargo.toml"), "--target", "x86_64-pc-windows-msvc", "--offline"]
            if command == "test":
                args += ["export_delta", "--", "--nocapture"]
            execute(args, f"delta-query-{command}.log", env=env)


if __name__ == "__main__":
    main()
