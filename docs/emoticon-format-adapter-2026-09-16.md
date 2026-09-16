# 远端表情格式适配迁移

## 基线与范围

本轮接在 SNS 适配迁移之后。此前根工程全量为 33 目标、2974 通过、0 失败、23 项原有忽略；这是本轮修改前的基线，不是本轮新代码的最终全量证明。没有提交、推送或处理真实账号。

原 `toolkit/emoticons/download.rs` 同时持有下载/发布流程和微信表情字节格式细节：AES-128-CBC 密钥解析、宽松去填充、图像标记及 HEVC 流起点选择。纯格式实现并不需要网络、账号或执行宿主，因此迁入现有微信表情适配器，不引入新业务模型或通用密码框架。下载/发布工作流随后迁至 `application/emoticons/download.rs`。

## 实际修改

- 新增 `adapters/wechat/emoticons/remote_format.rs`，拥有 decrypt、detect、hevc_stream；VPS/SPS 和查找辅助保持内部可见。原工作流实现删除，调用方直接引用适配器，无兼容转发。
- 下载、URL 回退、超时/大小限制、ffmpeg 监督、输出保护和发布仍在原工作流。CatalogSource、业务引用及 daemon 导出入口不变。
- CBC 改用库的原位接口，避免适配器额外要求 cbc alloc feature；算法、IV、错误消息和结果语义不变。仍调用现有 aes/cbc 库，不手写密码算法。
- 三项纯格式测试迁至 `remote_format_tests.rs`；增加 .NET 独立生成的固定密文向量，覆盖全部 1..16 填充及全填充结果，不仅依赖同库加解密 round-trip。工作流新增坏密文、空密文和全填充不发布反例。

## 不变语义

密钥必须为 16 字节，IV 等于密钥；六种 ASCII 空白仅能位于完整十六进制字节之间。24/32 字节密钥不属于该格式。块长错误仍拒绝；空密文仍可恢复为空，但工作流将不足四字节的结果报告为下载不可用且不发布。

只剥离长度 1..16 且尾字节全部相等的填充，其他尾部原样保留。这是现有媒体格式兼容规则，不是旧工具入口，不能以去兼容为由改成严格 PKCS#7。CBC 无认证，正确长度但错误的密钥未必报错；文件扩展名识别和 MD5 文件名也不是真实性认证。

HEVC 标记识别仍只检查原来的前 256 字节范围，流起点仍优先 VPS，即使 SPS 更早。抽出的字节切片不执行帧解码，真正 ffmpeg 操作保留在受监督执行边界。

## 验证与限制

检查使用当前 checkout 专属构建目录与 Windows x64 MSVC；合成 SQLite、人工密钥、本地测试 HTTP 服务，不访问私人媒体。Cargo 命令均使用 `--offline --locked --target x86_64-pc-windows-msvc`，测试附带 `-- --test-threads=1`：

- `cargo check --all-targets` 退出 0；`cargo test --bin wx emoticons`：34 通过、1 项原有 FFmpeg 条件忽略，无失败。
- `cargo test --test emoticons_runtime`：7 通过、无失败/忽略，使用本 checkout 的真实 CLI/daemon。
- `cargo test --manifest-path tests/fixtures/emoticons-download/Cargo.toml --all-targets`：模块目标 142 通过、2 项原有忽略；监督目标 3 通过，无失败。忽略项为外部 FFmpeg 条件和由父用例启动的进程 helper。包含实际子进程超时回收、输出限额及成功转换发布。
- 根工程 `cargo clippy --all-targets -- -D warnings` 与 `cargo fmt --all -- --check` 退出 0；最终 `scripts/check-fixtures.ps1 -Offline` 的 27 个夹具全部编译通过，所有日志均无默认 warning。矩阵不代表全部运行测试通过。
- 上述日志位于 C:/CodexLocal，前缀 wx-workbench-emoticon-adapter-，后缀分别为 check.log、tests.log、runtime.log、fixture-tests.log、clippy.log、fmt.log。最终矩阵日志为 wx-workbench-fixture-warnings-final-2.log。
- 额外 `cargo clippy --all-targets -- -W clippy::cognitive_complexity` 仍有 4 个生产热点：server::dispatch 49、monitor 26、latency 31、SNS export 26；8 个测试热点：delta_runtime 38、daemon-tasks/mcp 27、runtime_isolation 37/29、mcp_runtime 56、worker_keys_tests 27、web tests 26/35。日志同前缀 complexity.log。上次报告 server 为 50，其间 Extract 路由曾改动，减少 1 点不能归因于本次表情迁移；其他三项保持原分数。

没有改变外部命令、配置或磁盘格式，没有删除用户数据，不需要重新初始化。四个生产热点保留可追踪的路由、监控状态和发布顺序，测试热点保留完整安全场景，不为指标机械拆分。本轮没有再次执行根工程所有测试，定向通过不能替代最终全量验收。ASR 惰性密钥通道与大清单通信仍未完成，后续优先处理。
