# SNS 视频离线 WASM runtime

本模块承载旧 `weflow_wasm_keystream.js` 对应的 WxIsaac64 密钥流及视频前缀解码，已被原生 CLI 和 SNS 相册调用。
runtime 本身不启动 Node、不执行 JS、不联网、不读取账户、密钥文件或缓存；显式 `new` 只读取 WASM 资产。
调用方可以读取密钥文件、缓存，或按其授权策略下载媒体，不能把 runtime 的离线边界扩大为整个相册工作流永不联网。
没有重写 ISAAC 或其他密码学算法；真正的密钥流仍由供应商 WASM 产生。

## 当前入口与状态（2026-09-07）

```powershell
wx toolkit decode-sns-video encrypted.bin decoded.mp4 --key-file C:\account-workspace\video-key.txt
wx toolkit decode-sns-video plaintext.mp4 copied.mp4
```

输入和输出是必需位置参数；加密输入必须提供 UTF-8 `--key-file`（最多 1024 字节），明文 MP4 不需要密钥或初始化 runtime。
默认使用内嵌 WASM；可选 `--wasm` 仍要求同一受审计哈希，不是任意插件入口。
[单视频 CLI](../../cli/sns_video.rs) 只解码前 128 KiB、流式复制尾部，在头部验证后才创建输出暂存文件，拒绝覆盖已有输出。内存 API 的 256 MiB 限额不等于 CLI 整文件上限；后者没有该整文件限额，也没有完整容器/播放验证。

| 状态 | 证据与边界 |
| --- | --- |
| 主仓接线 | [sns/mod.rs](mod.rs) 已注册 runtime，[Cargo.toml](../../../Cargo.toml) 已声明 wasmi；[album.rs](album.rs) 为图片及视频使用 `VideoRuntime::bundled`，无需 Node 桥 |
| 当前主线自动化 | 精简后 Rust `1325 passed / 0 failed / 11 ignored`、个人 Web `61/0`；另有 8 次可选测试执行通过，check 通过但保留 9 警告。不是本模块独立测试数 |
| 验证边界 | 本轮仅静态校订文档，无 Cargo、网络或真实视频验证；真实账号、私人 key、任意播放器、模型/GPU、真实云服务未据此验收。企微排除目标，保留其既有实现和入口 |

最新状态与日志出处见 [Rust 迁移记录](../../../docs/rust-migration.md)、[系统架构](../../../docs/architecture.md)。

主线本轮文档同步验证：`C:/CodexLocal/wx-cli-doc-sync-tests.log` 终态退出 0，20 套件 `1325/0/11`；check 退出 0、9 警告，18 项 EXE help 检查通过。help 通过不代表真实视频可播放或部署验收，本文件维护者未重跑这些命令。

## 集成接口

```rust,ignore
let runtime = VideoRuntime::bundled(RuntimeLimits::default())?;
let plaintext = runtime.decode(caller_supplied_key, caller_supplied_data)?;
let stream = runtime.keystream(caller_supplied_key, requested_prefix_length)?;
```

- `new(&Path, RuntimeLimits)`：只读取显式 WASM 文件，校验大小和 SHA-256 后编译。
- `bundled(RuntimeLimits)`：加载编译时内嵌的同一 WASM，安装后不依赖 vendor 目录；该 WASM 仍是源码构建输入，不能删除。
- `keystream(&str, usize)`：接受 1..=25 MiB 字节，且不超过调用方 `input_bytes`；向上按 8 对齐，生成后整段反转、截取。扩展长度供相册图片使用，视频仍仅处理 128 KiB 前缀。
- `decode(&str, &[u8])`：已有 MP4 `ftyp` 头则原样返回，否则仅 XOR 前 128 KiB，保留后续字节，并验证 MP4 头。
- `VideoRuntimeError`：固定的错误类别，不含 key、guest 异常、输入内容或文件路径。
- `RuntimeLimits` 默认燃料硬上限 300,000,000，单 guest 内存 64 MiB，内存输入 256 MiB。实际燃料按对齐后的请求长度计算：128 KiB 内为 100,000,000，之后每字节加 8，最终受 300M 及调用方上限双重限制。
- 生成密钥流时，key 原始 UTF-8 输入上限 1024 字节且去空白后不得为空；WASM 文件上限 4 MiB，拒绝空密钥流/空视频。明文 MP4 直通不要求 key。
- 每个调用独立 Store；正常完成和调用失败后都使用 `zeroize` 擦除 guest 线性内存及未返回的宿主密钥流。调用方仍负责自己的 key/返回值生命周期，不能据此声称整个进程无残留。

主仓已注册模块并声明以下依赖，不再需要手动补接线：

```toml
wasmi = { version = "=0.46.0", default-features = false, features = ["std"] }
```

另使用仓库已有 `sha2 = "0.10"`、`zeroize = "1"`；测试使用已有 `serde_json`。
历史独立 harness 固定自己的 Cargo.lock；当前生产构建使用主仓清单和锁文件。

## 已核对 ABI

来源：`vendor/wechat-decrypt/sns_media_wasm` 下 README、两份 JS、实际 WASM 导入/导出及类注册探针，另核对 `export_sns_album.py` 的 `WasmKeystream` 和 `download_decrypt_video`。

支持的 WASM SHA-256：
`dca796bacec37d8522c7983b3945e5d579bd74164e3b21f0ebc773be6dfc8b6e`

- Emscripten 模块初始内存 32 MiB，导出 `memory`、`__wasm_call_ctors`、malloc/free 与 dynCall。
- 初始化提供空 `environ_sizes_get/environ_get`；忽略 atexit 注册，不读取机器环境。
- `_embind_register_class` 找到 `WxIsaac64` 并取得析构函数；构造与 `generate` 的 invoker/context 从注册回调捕获。
- 字符串为 `[u32 UTF-8 字节数][UTF-8 字节][NUL]`；通过 `dynCall_iii` 调用构造 invoker，通过 `dynCall_viii` 调用 generate invoker，通过 `dynCall_vi` 析构。
- JS `ASM_CONSTS[434460]` 是 `wasm_isaac_generate(ptr, n)`；只接受 `ii` 参数签名、单次回调和预期对齐长度，执行有界内存复制。
- 其他 embind 类型注册仅作初始化元数据；其他宿主功能默认 trap。没有 WASI 文件句柄、网络、FFmpeg 异步 I/O、JS eval 或时钟实现。
- 选择解释器 wasmi，避免为这个窄 ABI 引入 JIT、JS 引擎或通用 Emscripten 层。燃料包含初始化、分配、构造、generate 和析构；内存与表容量受 StoreLimits 限制。
- 此适配器严格绑定已审计 WASM 哈希；更新供应商二进制需重新探测及跑对照，不能简单删除哈希校验。

## 历史向量与回归入口

下面是初始独立迁移时的向量来源与复现命令，本轮未执行；当前扩展限额和异常用例见 [video_runtime_tests.rs](video_runtime_tests.rs)，主线结果见开头。
原始向量均为公开合成输入，原 Node 包装器 `--stdio` 是基准。
`tests/fixtures/sns-video-native/generate-vectors.cjs` 生成 `vectors.json`：
key 为 `0`、`1`、`-1`、`18446744073709551615`、`00042`、带空格的 `42`；
长度覆盖 1、7、8、9、16、1023、1024、1025、131071、131072，共 35 组逐字节比较。
固定向量的常规 Rust 测试不需要 Node；重新生成时才需要 Node。

```powershell
node tests/fixtures/sns-video-native/probe.cjs
node tests/fixtures/sns-video-native/generate-vectors.cjs
cargo check --manifest-path tests/fixtures/sns-video-native/Cargo.toml --target x86_64-pc-windows-msvc
cargo test --manifest-path tests/fixtures/sns-video-native/Cargo.toml --target x86_64-pc-windows-msvc
```

额外测试覆盖前缀边界、后缀保留、明文直通、错误 MP4、缺失/未知模块、非法及超长 key、BOM trim、空/超限输入、低燃料、低内存、异常后再次使用及不同 key 隔离。

## 明确边界

- runtime 不承担下载、缓存或账号选择；原生 CLI、相册及父模块注册已完成。它替换特定 Node 密钥流业务，不代表供应商整个 JS/WASM 软件栈都被重写。
- 不实现该 WASM 中的 FFmpeg 或其他导出功能，因为原 Node 业务路径未调用它们。
- 未用真实视频、私人 key 或账户数据验证；MP4 验证与旧代码一致，仅检查长度及 `ftyp`，不等同于完整容器验证。
- 不复刻 Node 启动、JSON stdio 协议或任意 JS 类型强制转换，Rust 接口显式接收字符串和整数长度。
- 不支持任意替代 WASM；不声称覆盖 C++ 数字解析器所有异常输入或所有 Unicode 边界。
- 独立 harness 记录是历史证据；当前整仓集成与可选执行由主线记录证明，本轮文档任务没有重新运行它们。
