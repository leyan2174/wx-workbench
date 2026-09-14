# Rust OpenAI 兼容转录模块

规范名称是 `openai_compatible`；`openai` 与 `explicit-open-ai` 为兼容别名。
共享解析与各入口授权边界见 [BACKENDS.md](BACKENDS.md)，命名更新不迁移 API 凭据。

## 接入边界

`openai.rs` 负责原生 HTTP multipart 转录；模块自身不读取环境变量、配置文件、音频文件、聊天或密钥文件，不执行 SILK 解码或缓存回写。它已通过 `asr/mod.rs` 接入 CLI、固定账号批处理和 MCP 宿主；daemon 内的宿主策略执行器在授权后读取显式凭证并调用此模块。构造客户端不会上传；只有调用 `transcribe_wav(audio, true)` 才可能发送请求。

后端一旦选定，缺 key、配置错误或调用失败均不自动回退 local。不能从配置、模型或缓存推断上传授权；`--allow-upload` 明确许可本次文件或整个批次将解码 WAV 发往所选服务端。

| 宿主入口 | 选择、凭证与默认值 |
| --- | --- |
| 原生单文件/清单/数据库 CLI，普通 MCP | `--backend explicit-open-ai --allow-upload --openai-base-url URL --openai-model MODEL --api-key-file FILE` 全部显式提供；不读取环境或默认 key。UTF-8 key 文件最多 16384 字节，trim 后传入；`--language auto` 省略 language，`--timeout-seconds` 默认 120 且须大于 0 |
| `transcribe-chat` 等配置式兼容批处理 | 固定账号 `transcription_backend="openai"` 或 `--backend explicit-open-ai` 选择云端，仍须 `--allow-upload`。URL 默认 `https://api.openai.com/v1`，模型默认 `whisper-1`，对应 CLI 参数可覆盖。凭证优先 `--api-key-file`，其次配置指定 `openai_api_key_env`，仅该字段缺失时才用旧 `openai_api_key`；指定环境变量名无效、未设置、非 Unicode、空或超过 16384 字节时失败，不退回明文 key |
| 批处理加 `--explicit-backend` | 完全使用第一行的显式参数构造规则，不使用配置式默认 URL/模型或凭证 |

云端拒绝 `--whisper-binary`、`--whisper-model`、`--threads`、`--temp-root`。显式后端在访问音频/凭证前检查授权；配置式批处理先读取固定配置，再在使用凭证及读取音频前检查授权。`--configured-local-python` 仅允许 MCP 本地桥，不接受云端混用。

## 稳定接口

- `OpenAiConfig { base_url: String, model: String, language: Option<String>, api_key: String, timeout: Duration, max_audio_bytes: usize }`
- `OpenAiTranscriber::new(OpenAiConfig) -> Result<OpenAiTranscriber, OpenAiError>`
- `OpenAiTranscriber::transcribe_wav(&self, audio: &[u8], upload_authorized: bool) -> Result<Transcription, OpenAiError>`
- `OpenAiTranscriber::tighten_timeout(&mut self, remaining: Duration) -> Result<(), OpenAiError>`：只收紧后续请求超时，零预算拒绝；MCP 在后端构造及 IPC 后按真实剩余期限调用
- `Transcription { text: String, language: String }`
- `OPENAI_AUDIO_LIMIT_BYTES = 25 * 1024 * 1024`

本模块不提供默认配置，避免意外启用。宿主默认值与必填参数见上表，宿主音频上传上限为 25MiB。`base_url` 是 API 根路径而非完整转录端点；末尾斜线可有可无，自动追加 `/audio/transcriptions`，兼容服务路径前缀原样保留。`Some(language)` 作为 multipart language 字段传入，`None` 则省略。

请求固定发送 `response_format=verbose_json`、`model` 及 `file`，音频文件名固定为 `audio.wav`，MIME 为 `audio/wav`。仅接收已解码 WAV 字节，调用者负责格式正确性；模块不验证 WAV 容器。返回文本执行 Unicode trim；缺失/null language 返回 `unknown`，null text 返回空字符串。缺失 text、错误类型或无效 JSON 作为 `InvalidResponse` 拒绝，较旧 Python SDK 的缺失 text 容错更严格。

## 安全与失败语义

- 必须显式传入上传许可；拒绝空音频、超出配置上限或超过硬上限的音频，均在建立 HTTP 请求前完成。
- 只接受 HTTPS；为本地兼容服务和 mock 允许数字回环地址 HTTP (`127.0.0.1`、`[::1]`)，不允许普通主机明文 HTTP。拒绝 URL 用户名、密码、查询和 fragment。
- 禁止重定向，禁止系统代理自动读取，不关闭 TLS 验证。没有应用层重试，也不依赖 OpenAI SDK。
- 客户端 timeout 同时约束上传和读取响应，connect_timeout 同值。响应正文读取上限 1 MiB，超限返回 `ResponseTooLarge`。
- 非 2xx 返回 `Http { status, json_error }`；`json_error` 仅标识 JSON 中存在 error 字段。401/429 显示固定鉴权/限流提示，其余保留状态码。不输出服务端 message/type/code、原始 JSON、URL 或底层错误链，以防回显 key 或音频。
- 配置、客户端及转录结果 Debug 均隐藏字段；Authorization 标记 sensitive。模块无日志输出。配置及请求凭证仍驻留内存，不承诺安全擦除；调用者不得自行打印配置字段或返回转录内容。

## 依赖与注册

当前共享 Cargo.toml 已声明 reqwest，父模块已注册 `pub mod openai;`。所需特性为：

```toml
reqwest = { version = "0.12", default-features = false, features = ["blocking", "multipart", "rustls-tls"] }
```

版本以当前 Cargo.toml/Cargo.lock 为准，不需要 reqwest 的 json feature。模块内部通过 `#[path = "openai_tests.rs"]` 注册测试，不要重复注册。这是 blocking API，Tokio 异步调用方须通过 `spawn_blocking` 或专用同步线程完成客户端创建、调用和销毁，不能直接在异步运行时线程内使用。

## 测试与兼容性

`openai_tests.rs` 使用本机回环服务和合成音频、凭证，覆盖 multipart 字段与原始字节、语言省略、路径前缀、预检零连接、尺寸限制、HTTP 错误、禁止重定向、响应超时和凭据不回显。按[测试说明](../../../tests/README.md)运行 `cargo test --bin wx toolkit::asr::openai`。

兼容服务必须支持 `verbose_json`；不支持时返回有类型的 HTTP 错误，不尝试其他协议。此模块不落盘，成功缓存由 `cached/receipt` 管理。端点、模型、语言和音频上限参与缓存身份，凭据不参与；缓存命中前仍检查上传授权。回环测试不代替真实云服务或模型质量验证。
