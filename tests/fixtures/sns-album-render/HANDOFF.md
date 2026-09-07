# SNS 相册纯渲染层

> 2026-09-07 文档核对：`album_render` 已注册，并由 `sns/album.rs` 调用。独立 harness、oracle、预览和日志保留；下面“未执行”“主线程仍需”等描述记录该次交付的验收边界，不是当前全仓状态。本次没有执行测试或浏览器验收。

## 主线程接线

- `src/toolkit/sns/mod.rs` 已声明 `pub(crate) mod album_render;`，无需重复注册。
- `album_render::render(user: &str, posts: &[serde_json::Value]) -> anyhow::Result<Rendered>` 返回 `html: String` 和 `album_posts: usize`。
- `album_render::safe_stem(value: &serde_json::Value, fallback: &str) -> String` 保持旧 Python 文件名转换语义，可供主编排复用；它不是输出目录安全策略，主线程仍应限制输出目录和 Windows 保留文件名。
- 生产模块没有文件系统、环境、网络或进程访问。媒体必须为 `images/<filename>` 或 `videos/<filename>`，按媒体类型匹配目录；不检查文件存在、内容、符号链接或重解析点，这些属于下载/发布层的职责。
- 远端路径、绝对路径、跨层路径、控制字符、Windows 保留名称和非法路径被过滤。文件名按 UTF-8 URL 编码，包括 `%/#/?`，百分号不会变为路径穿越。
- 旧布局/CSS、Emoji、内容清洗、年份导航首次出现顺序以及年份标题切换规则保留。旧输入中年份非连续时会产生重复年份 id，这里没有擅自改变该行为。
- 现有 `sns/export.rs::escape` 是私有函数，且单引号实体不同。这里只保留小型 Python 兼容 escape；未复制其他渲染器或修改辅助函数可见性。

## 验证

仅 Windows MSVC。现行生产模块可从仓库根运行 `cargo test --bin wx toolkit::sns::album_render::tests -- --nocapture`；这条命令不是已执行结果。

以下为原独立验收命令与范围；该次未执行全仓测试。生成 oracle/预览的命令会写出文件，复核历史证据时不要无意重建或覆盖。

```powershell
$env:LIBCLANG_PATH='C:\CodexLocal\build-tools\libclang\clang\native'
cargo test --manifest-path tests/fixtures/sns-album-render/Cargo.toml --target x86_64-pc-windows-msvc --target-dir C:\CodexLocal\sns-render-target
cargo check --target x86_64-pc-windows-msvc
python tests/fixtures/sns-album-render/oracle.py --assets
```

独立 harness 直接编译生产 Rust 文件，无需修改主 Cargo 或 mod.rs。AST 白名单只提取九个纯函数和 Emoji 常量，build_html 的写入被内存 Sink 接管，不导入旧模块，不执行 main、下载或真实网络。测试覆盖全部 Emoji、未知 Emoji、Python 假值及标量转换、完整 HTML golden、文本/图片/视频、空时间线、年份、注入、路径、尺寸异常、Unicode 数字和大整数。

## 浏览器验收

打开本目录 `preview.html`。其内容全部合成，图片 `images/synthetic.png` 是标准库生成的 480×320 RGB PNG；`videos/placeholder.mp4` 是明确标注的空占位文件，仅检查播放器布局，不用于验证解码/播放。没有外部资源。

主线程仍需执行桌面和手机浏览器验收；本子任务未宣称已完成视觉验收。运行精准测试会重新生成 Rust 版本 `preview.html`。

## 文件清单

- `src/toolkit/sns/album_render.rs`
- `src/toolkit/sns/album_render_tests.rs`
- `tests/fixtures/sns-album-render/Cargo.toml`、`Cargo.lock`
- `tests/fixtures/sns-album-render/oracle.py`
- `tests/fixtures/sns-album-render/preview.json`、`preview.html`
- `tests/fixtures/sns-album-render/images/synthetic.png`
- `tests/fixtures/sns-album-render/videos/placeholder.mp4`
- 本交接文件

日志位于 `C:\CodexLocal\sns-render-test.log`、`C:\CodexLocal\sns-render-check.log` 和 `C:\CodexLocal\sns-render-oracle.json`。
