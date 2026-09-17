# SNS WxIsaac64 密钥流宿主

`sns_keystream.rs` 是微信媒体适配器，承载由调用方提供、固定哈希 WASM 所产生的 WxIsaac64 密钥流，供 SNS 图片与视频使用；视频前缀恢复不是视频帧解码。原生 CLI 的执行宿主和 SNS 相册工作流直接调用此适配器，旧工作流目录不保留转发模块。
runtime 本身不启动 Node、不执行 JS、不联网、不读取账户、密钥文件或缓存；显式 `new` 只读取 WASM 资产。
调用方可以读取密钥文件、缓存，或按其授权策略下载媒体，不能把 runtime 的离线边界扩大为整个相册工作流永不联网。
没有重写 ISAAC 或其他密码学算法；真正的密钥流仍由供应商 WASM 产生。

## 入口

```powershell
wx media video decode encrypted.bin decoded.mp4 --key-file video-key.txt --wasm authorized-wasm-video-decode.wasm
wx media video decode plaintext.mp4 copied.mp4
```

输入和输出是必需位置参数；加密输入必须提供 UTF-8 `--key-file`（最多 1024 字节），明文 MP4 不需要密钥或初始化 runtime。
公开源码和发布包不携带许可证状态不明的 WASM。加密输入必须同时通过 `--wasm` 提供用户已获授权的本地资产；它仍须匹配固定哈希，不是任意插件入口。省略时会明确返回资产不可用，且不会联网下载或搜索本机文件。哈希固定资产身份，不代表独立审计了 WASM 内部算法。明文 MP4 的直通路径不读取该资产。
[单视频操作](../../../daemon/operations/sns_video.rs) 只解码前 128 KiB、流式复制尾部，在头部验证后才创建输出暂存文件，拒绝覆盖已有输出。内存 API 的 256 MiB 限额不等于 CLI 整文件上限；后者没有该整文件限额，也没有完整容器/播放验证。


## 集成接口

```rust,ignore
let runtime = SnsKeystream::new(authorized_asset_path, RuntimeLimits::default())?;
let plaintext = runtime.restore_video(caller_supplied_key, caller_supplied_data)?;
let stream = runtime.keystream(caller_supplied_key, requested_prefix_length)?;
```

- `new(&Path, RuntimeLimits)`：只读取显式 WASM 文件，校验大小和 SHA-256 后编译。
- `bundled(RuntimeLimits)`：公开默认构建固定返回 `AssetUnavailable`。仅在内部来源核验使用 `sns-wasm-test-asset` 特性时读取工作树中的审计副本；该特性不是公开分发资产的机制。
- `keystream(&str, usize)`：接受 1..=25 MiB 字节，且不超过调用方 `input_bytes`；向上按 8 对齐，生成后整段反转、截取。扩展长度供相册图片使用，视频仍仅处理 128 KiB 前缀。
- `restore_video(&str, &[u8])`：已有 MP4 `ftyp` 标记则原样返回，否则仅 XOR 前 128 KiB，保留后续字节，并检查该格式标记。它不是完整容器验证、MAC 或真实性认证，也不保证检测所有错误密钥。
- `KeystreamError`：固定的错误类别，不含 key、guest 异常、输入内容或文件路径。
- `RuntimeLimits` 默认燃料硬上限 300,000,000，单 guest 内存 64 MiB，内存输入 256 MiB。实际燃料按对齐后的请求长度计算：128 KiB 内为 100,000,000，之后每字节加 8，最终受 300M 及调用方上限双重限制。
- 生成密钥流时，key 原始 UTF-8 输入上限 1024 字节且去空白后不得为空；WASM 文件上限 4 MiB，拒绝空密钥流/空视频。明文 MP4 直通不要求 key。
- 每个调用独立 Store；正常完成和调用失败后都使用 `zeroize` 擦除 guest 线性内存及未返回的宿主密钥流。调用方仍负责自己的 key/返回值生命周期，不能据此声称整个进程无残留。

根工程注册模块并声明以下依赖：

```toml
wasmi = { version = "=0.46.0", default-features = false, features = ["std"] }
```

另使用仓库已有 `sha2 = "0.10"`、`zeroize = "1"`；测试使用已有 `serde_json`。
生产构建使用主仓清单和锁文件，但不启用 `sns-wasm-test-asset`。

## 已核对 ABI

来源与许可边界见 [assets/README.md](assets/README.md)。ABI 依据为固定的 WASM 导入/导出探针、JS 包装器行为和相册调用路径；仓库不分发 Python/Node 参考源码。

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

## 向量与回归入口

扩展限额和异常用例见 [sns_keystream_tests.rs](sns_keystream_tests.rs)，运行前按[测试说明](../../../../tests/README.md)配置依赖。
原始向量均为合成输入，以 Node 包装器输出为独立基准并固定提交。回归使用固定向量，不依赖参考源码目录中的重生成器。需要执行 WASM 的向量测试只用于持有合法审计副本的内部核验，不属于公开源码默认测试。`vectors.json` 的输入包括：
key 为 `0`、`1`、`-1`、`18446744073709551615`、`00042`、带空格的 `42`；
长度覆盖 1、7、8、9、16、1023、1024、1025、131071、131072，共 35 组逐字节比较。
固定向量测试不需要 Node，但需要显式启用内部特性并在预期路径提供已授权资产。默认测试只验证缺失和未知资产会被拒绝。

```powershell
cargo check --manifest-path tests/fixtures/sns-video-native/Cargo.toml --target x86_64-pc-windows-msvc
cargo test --manifest-path tests/fixtures/sns-video-native/Cargo.toml --target x86_64-pc-windows-msvc
```

额外测试覆盖前缀边界、后缀保留、明文直通、错误 MP4、缺失/未知模块、非法及超长 key、BOM trim、空/超限输入、低燃料、低内存、异常后再次使用及不同 key 隔离。

## 明确边界

- runtime 不承担下载、缓存或账号选择；原生 CLI、相册及父模块注册已完成。它替换特定 Node 密钥流业务，不代表供应商整个 JS/WASM 软件栈都被重写。
- 公开默认构建中的相册工作流没有接收外部 WASM 的参数；需要 WxIsaac64 密钥流的远端图片或视频会报告引擎不可用。既有明文媒体、无需该密钥流的缓存路径和单视频显式 `--wasm` 路径不受此限制。
- 不实现该 WASM 中的 FFmpeg 或其他导出功能，因为原 Node 业务路径未调用它们。
- MP4 验证仅检查长度及 `ftyp`，不等同于完整容器验证或播放器兼容性验证。
- 不复刻 Node 启动、JSON stdio 协议或任意 JS 类型强制转换，Rust 接口显式接收字符串和整数长度。
- 不支持任意替代 WASM；不声称覆盖 C++ 数字解析器所有异常输入或所有 Unicode 边界。
