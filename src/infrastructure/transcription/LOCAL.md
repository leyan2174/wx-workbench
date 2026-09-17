# 本地 ASR：whisper.cpp 与可选 Python 推理桥

规范名称及授权边界见 [ASR 后端](../../../docs/asr-backends.md)。MCP 配置式 Python
只接受 `python_whisper`，并要求宿主启用对应入口。

## 入口与默认值

Rust 已负责音频校验/SILK 解码、数据库关联、缓存、回写和进程监管；`application/transcription/mod.rs` 组合两个本地后端。Python 只保留 Whisper/PyTorch 推理桥，不运行旧 `mcp_server.py` 或批量导出脚本，也不是识别失败后的自动回退。

| 入口 | 选择与默认值 |
| --- | --- |
| `wx toolkit transcribe-audio-native INPUT`、`transcribe-chat-native INPUT OUTPUT --media-manifest FILE --media-root DIR`、`transcribe-database-native` | 共用 `BackendArgs`：默认 `--backend whisper_cpp`，必须给 `--whisper-binary` 和 `--whisper-model`；`--language auto`、`--timeout-seconds 120`，`--threads` 省略时自动且最多 8；CLI 强制 JSON 结果模式；可选 `--temp-root` |
| `wx toolkit transcribe-chat INPUT [OUTPUT]` 及复用批处理的导出入口 | 实际请求转录后读取固定账号配置；必须明确配置 `transcription_backend`。`python_whisper` 的 `local_whisper_model` 缺失时默认 `base`。`--explicit-backend` 改用上行显式原生参数；输出省略时为同目录 `<输入主名>_transcribed.json`；`--asr-cache-name` 默认 `batch-transcriptions.json` |
| 配置式 `transcription_backend="whisper_cpp"` | 程序取 `whisper_cpp_binary`；模型取 `whisper_cpp_model`；配置相对路径基于配置目录。缺模型或空模型时，依次从用户目录 `whisper-models`、`models`、`Downloads` 选择首个目录内按文件名排序的 `ggml-*.bin`，不下载。语言配置缺省 `zh`，线程缺省/0 为原生自动且最多 8；`--threads` 和非 `auto` 的 `--language` 可覆盖，`--whisper-binary/--whisper-model` 可显式固定路径 |
| `wx mcp` | 默认本地方式是显式 whisper.cpp 路径；Python 必须由宿主加 `--configured-local-python`，并要求固定配置中明确有 `transcription_backend="python_whisper"`；模型字段缺省 `base`。拒绝混用 cpp 路径、云参数和用户 `--temp-root`，宿主创建请求独占临时目录；允许语言、线程、超时及显式缓存参数 |

线程和超时必须为正值。普通原生 CLI/MCP 不读取旧配置来发现 cpp 程序或模型；旧模型搜索仅属于配置式兼容批处理。云端选择与上传授权见 [OPENAI.md](OPENAI.md)，缓存及恢复见 [CACHE.md](../../application/transcription/CACHE.md)、[WRITEBACK.md](../../application/transcription/WRITEBACK.md)。

## whisper.cpp API

稳定入口为 `transcribe(&LocalConfig, &Path) -> anyhow::Result<Transcription>`；
`LocalConfig::new(executable, model)` 强制调用方提供本地文件路径，不搜索 PATH、
不下载、不调用 Python/Node/OpenAI。相对路径基于调用时工作目录解析。
默认 auto 语言、最多 8 个 CPU 线程、120 秒超时、文本输出。
`OutputFormat::Json` 使用 whisper.cpp `-oj`；文本使用 `-otxt`。
两者通过 `-of` 将结果写入每次调用独有的临时目录。

返回字段：`text: String`、`language: String`、`backend: String`。
JSON 支持 whisper.cpp 的 `transcription[].text` 与 `result.language`，
亦接受顶层 text/language。保留片段中的原始空格，仅 trim 最终文本。
文本模式无法获知 auto 检测语言，因此返回 unknown，不伪造检测结果。
有效空结果代表静音；缺失结果文件、非零退出码、坏 JSON 不视为静音。

依赖复用仓库已有 anyhow、serde(derive)、serde_json、tempfile 与 windows。
父模块已适配统一 `Transcription` 并接入文件/字节转录、清单回写、数据库、批处理及 MCP。
调用是阻塞式，应由异步上层使用 spawn_blocking。输入应为现有音频模块
产出的 whisper.cpp 支持音频；本模块不校验音频编码、不重复 SILK 解码。
上层继续负责账号隔离、缓存身份、批处理和导出持久化时机；摘要与 receipt 不是数字签名。

Windows 使用 CREATE_NO_WINDOW | CREATE_SUSPENDED，不经过 shell。先将挂起进程加入
启用 KILL_ON_JOB_CLOSE 的 Job Object，再恢复其主线程；绑定失败则终止直接进程并报错。
超时、超限、读取失败、正常退出均终止 Job 并等待活动进程归零，回收
已纳管的子孙进程。目录删除最多重试 1 秒，失败明确报错。
`windows_supervision.rs` 仅保留 ASR 诊断适配，Job、管道读取和回收复用
`windows_process::managed`，与直接 FFmpeg、daemon worker 使用相同实现；仅
`PeekNamedPipe` 通过 kernel32 FFI 调用。
每次显式回收使用独立的 2 秒预算，覆盖 Job 活动进程归零和直接子进程退出，
不会无限 wait；回收失败不宣称终止已确认。共享 Job 可随 Python worker 跨线程移动。
内层 Job 不允许 breakaway；终止内层不会终止外层 worker，终止外层会回收内层后代。
Python 单次调用的锁等待、冷启动与识别共用同一截止时间，不为每个阶段重置完整预算。

需要调整资源预算时使用：

```rust
transcribe_with_limits(&config, audio, ResourceLimits {
    max_temp_bytes: 32 * 1024 * 1024,
    max_response_bytes: 16 * 1024 * 1024,
    max_stream_bytes: 4 * 1024 * 1024,
    max_temp_entries: 4096,
})
```

以上是默认值。每轮递归累计所有临时文件逻辑长度和条目数，检查结果文件
响应大小，拒绝链接和 Windows reparse point。stdout/stderr 分别公平读取，
仅累计两者合计字节数，超限终止；不保存日志、不回显进程输出。非零退出
仅报告状态码，避免 stderr 包含识别全文、模型内容或凭据。管道通过
PeekNamedPipe 查询可读量，每管道每轮最多读 64KiB，不等待 EOF，不因派生进程继承管道而无限阻塞。

限制必须明确：这是约 10ms 轮询的终止阈值，不是文件系统硬配额；高速写入
可能短暂超调，目录扫描也有开销。只统计专属临时目录，不限制可执行文件
主动写到其他位置。挂起启动消除了绑定前运行子进程代码的窗口，但 Job 不是安全沙箱。
只运行可信 whisper.cpp，不执行不可信包装程序；临时根
必须为可信账户隔离目录。非 Windows 平台拒绝运行。

## Python 推理桥

`LocalPythonConfig::discover` 固定解释器发现结果、环境和模型根，但不启动 Python；真正需要引擎身份或识别时才启动 worker。解释器按 `VIRTUAL_ENV` 中的 Windows 解释器、配置目录或当前可执行文件附近的 `.venv`、最后 `PATH` 中的 `python` 依次发现。模型相对路径始终基于选定的配置目录，不依赖仓库内的 Python 工具目录或其专用环境变量。

解释器以 `-S -u -B -c` 运行内嵌桥，加入 Job 后接收单字节握手，再执行 `site.main()`、导入 Whisper/PyTorch。命名模型保留 Whisper 按需下载权重和摘要检查；本地权重路径须存在。首次实际识别时加载模型，后续请求复用常驻模型；CUDA 可用时沿用 Whisper/PyTorch 的 CUDA 选择，否则 CPU。Rust 负责进程编排，GPU 推理由 Whisper/PyTorch 执行。

桥不上传音频，但命名模型可能联网下载；保留的代理、Python/site、CUDA 环境及第三方包均属于可信部署边界，不能称为无网络沙箱。音频使用专属临时 WAV，不创建临时 SILK。请求 JSON 小于 16KiB、启动配置最多 8KiB；每次响应等待阶段 stdout/stderr 合计上限 1MiB，响应上限 1MiB，专属目录累计 256MiB/4096 条目。约 10ms 轮询，非硬配额，不约束目录外模型缓存。

两个本地后端共用 `windows_supervision`，但保持各自协议、资源阈值及清理顺序。Python 在错误或 worker 释放时回收进程，成功请求保持 worker 供复用；临时目录清理最多重试 1 秒。MCP 使用绝对截止时间约束初始化、缓存身份和识别，只能收紧预算；普通配置的 120 秒不构成整个聊天批次总时限。Job 在 spawn 后绑定，仍不承诺消除绑定前逃逸窗口。

## 测试

从仓库根目录按[测试说明](../../../tests/README.md)运行 `cargo test --bin wx infrastructure::transcription::local::`。本地后端测试使用合成程序验证参数、空格路径、文本/JSON、空结果、非零退出、超时及临时目录清理。

资源测试覆盖双管道持续输出、响应超限、文件及目录条目超限、挂起，以及父进程结束后的子孙进程回收。合成心跳停止和临时目录清理用于检查回收行为，不代表真实模型、GPU 或识别质量已经验证。需要安装依赖、下载模型或人工确认的项目单列处理。
