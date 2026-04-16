# wx-cli Project Rules

## After Every Code Change

**Rust 代码改动后，必须立刻运行：**

```bash
cargo check
```

不允许在 `cargo check` 通过之前提交或推送。

**改动涉及跨平台代码（`#[cfg(...)]` / `Cargo.toml` dependencies）时，额外运行：**

```bash
cargo check --target x86_64-unknown-linux-gnu
cargo check --target x86_64-pc-windows-msvc
```

这两条命令在 macOS 上可以执行（只做类型检查，不需要链接器），用于提前暴露 Linux/Windows 特有的编译错误。

## Cargo.toml 修改规则

- 修改版本号后，必须运行 `cargo update --workspace` 更新 Cargo.lock
- 添加/移动 `[target.'cfg(...)'.dependencies]` section 时，确认后续依赖没有被意外归入该 section（TOML section 持续到下一个 header）
- 改完后运行 `cargo check` 验证

## Git 规则

- 每次 commit 后必须 push（`git push wx-cli main`）
- 打 tag 前确认 `cargo check` 和 `cargo update --workspace` 都已完成
- remote 使用 `wx-cli`（SSH），不用 `origin`

## 平台兼容性检查清单

改动以下内容时必须做跨平台 check：

- [ ] `libc::` 调用 → 确认函数在 Linux 和 macOS 都存在（`__error` 是 macOS 专属，用 `std::io::Error::last_os_error()` 代替）
- [ ] `#[cfg(unix)]` 块 → unix 包括 macOS 和 Linux，不能用 macOS 专属 API
- [ ] `Cargo.toml` dependency section 顺序 → 检查是否有 dep 意外落入 target section
- [ ] Windows named pipe 代码 → 确认函数都已定义，trait import 齐全

## CI 结构

```
check job（ubuntu）
  └── cargo check --target linux-x86, linux-arm64, windows-x86
        ↓ 通过后
build jobs（5平台并行）
        ↓ 全部通过后
publish-npm job
```
