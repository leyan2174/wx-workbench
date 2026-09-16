# 转录分层与 Toolkit 删除

## 基线与问题

本轮基于 Windows x64 MSVC 当前脏工作区，使用独立 `CARGO_TARGET_DIR=C:/CodexLocal/wx-workbench-target-20260916`。迁移前 `src/toolkit` 剩余 27 个文件，全部属于 ASR，但其中同时混放应用编排、具体进程/网络后端和公共请求校验。

具体反向依赖包括：service 请求校验引用 `toolkit::asr::backend`；Python 后端返回应用 `Transcription`，并调用应用缓存模块的文件摘要函数。直接把目录改名会保留错误依赖方向。

## 实际修改

- `src/application/transcription`：转录管线、离线媒体清单、缓存、回执、回写、受控音频准备和数据库批次。
- `src/infrastructure/transcription`：whisper.cpp、Python Whisper、OpenAI-compatible 客户端、Windows 监督适配和固定文件摘要。
- `src/service/operation_requests/asr_backend.rs`：后端身份和无 IO 参数校验；身份不再依赖调用入口。
- daemon、MCP、查询、service 和独立 fixture 直接引用真实所有者；测试不再伪造 `toolkit::asr` 模块。
- 删除 `src/toolkit`、根 `mod toolkit` 和所有 ASR 兼容转发路径。正式 `wx toolkit` 命令分组不变。
- 后端 wire/config/cache 身份统一为 `whisper_cpp`、`python_whisper`、`openai_compatible`。删除会按入口映射为不同引擎的 `local`、OpenAI 别名、缺失配置默认值和 `Entry` 分类。
- 应用后端枚举同步改为 `WhisperCpp`、`PythonWhisper`、`OpenAiCompatible`；配置模式只服从固定账号配置，显式模式由 `--explicit-backend` 选择。
- 配置式 OpenAI-compatible 凭据只接受 `openai_api_key_env`；删除 `openai_api_key` 明文回退及环境报告中的旧明文探测。
- 新缓存只接受三个规范后端标签；旧 `local`、`openai`、`legacy-python-local` 缓存明确拒绝，不自动迁移。

## 边界与复杂度

| 指标 | 修改前 | 修改后 |
| --- | ---: | ---: |
| `src/toolkit` 文件 | 27 | 0 |
| 根生产 `mod toolkit` | 1 | 0 |
| service 对 Toolkit 执行模块依赖 | 2 个文件 | 0 |
| Python 后端对应用结果/缓存依赖 | 2 | 0 |
| ASR 后端输入名称 | 8 个规范名/别名及入口相关解释 | 3 个入口无关规范名 |
| 配置式云凭据来源 | 环境变量名或明文回退 | 仅环境变量名 |

本轮没有机械拆分 local/Python/OpenAI 的执行控制流。进程、网络、超时、取消和资源上限算法原样归位；唯一新增转换是在应用组合边界把 Python 后端结果投影为统一 `Transcription`。因此不声称函数圈复杂度下降，结构复杂度下降由上述依赖和模块所有权指标证明。

## 验证

- `cargo check --locked --target x86_64-pc-windows-msvc --all-targets`：通过。
- `cargo test ... --bin wx application::transcription -- --test-threads=1`：74 通过。
- `cargo test ... --bin wx infrastructure::transcription -- --test-threads=1`：34 通过。
- `cargo test ... --test asr_video_security -- --test-threads=1`：340 通过，2 项既有条件忽略。
- `cargo test ... --test mcp_audio_adapter -- --test-threads=1`：186 通过。
- 独立 `asr-local`：24 通过、1 条件忽略；`asr-cache-security`：345 通过、3 条件忽略。
- 独立 `mcp-voice-host`：378 通过、2 条件忽略，宿主/关闭测试 9 通过；`mcp-voice-host-security`：19 通过、1 个 Windows 符号链接权限条件忽略。
- `cargo fmt --all -- --check`、all-target Clippy `-D warnings`、`git diff --check`：通过。Git 行尾策略提示不是 Rust 编译告警，未批量改写无关文件。

名称与凭据收敛后追加验证：

- `cargo check --locked --target x86_64-pc-windows-msvc --all-targets`：通过，无编译告警。
- `cargo test ... --bin wx application::transcription`：74 通过。
- `cargo test ... --bin wx daemon::operations::asr`：24 通过。
- `cargo test ... --bin wx daemon::mcp_service::voice::configured_local_tests`：7 通过。
- `cargo test ... --bin wx service::settings`：4 通过。
- `cargo test ... --bin wx cli::request_conversion_tests`：3 通过。
- `cargo test ... --test asr_video_security`：340 通过，2 条件忽略。
- `cargo test ... --test mcp_voice_runtime`：7 通过。
- 独立 `asr-cache-security`：345 通过，3 条件忽略。
- 独立 `mcp-voice-host`：库 378 通过、2 条件忽略；宿主和关闭测试 9 通过。
- 独立 `mcp-voice-host-security`：库 289 通过、1 条件忽略；审计 19 通过、1 条件忽略。首次编译发现安全 fixture 的音频 wrapper 缺少测试专用真实函数重导出，补齐 `pcm24k_to_wav` 与 `decode_silk_to_pcm` 后通过；未添加 mock。

## 移除的兼容机制

- `--backend local`：删除；whisper.cpp 使用 `whisper_cpp`，Python 使用 `python_whisper`。
- `openai`、`explicit-open-ai`、`ExplicitOpenAi`：删除；唯一替代为 `openai_compatible`。
- 缺失 `transcription_backend` 时默认 Python：删除；账号配置必须明确选择后端。
- `openai_api_key` 明文配置：删除；配置式云转录只保存并读取 `openai_api_key_env` 指定的环境变量名，显式入口继续使用受限 key 文件。
- 旧 ASR 缓存后端标签：不再读取；用户需重新生成转录缓存。不会删除用户磁盘上的旧缓存或配置。

测试只使用合成数据库、合成音频、本机回环 HTTP 和临时目录。未读取真实微信账号，未运行真实密钥提取，未执行真实模型识别或云上传。

## 未验证与保留问题

- 真实 whisper.cpp/Python 模型的识别质量、真实云服务和真实微信版本覆盖未验证。
- 根工程全量测试与完整 fixture 矩阵尚未在本切片运行；最终交付前仍需执行。
