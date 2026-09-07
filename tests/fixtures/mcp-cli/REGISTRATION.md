# 主线程注册与隔离验收

> 2026-09-07 文档核对：生产 CLI/MCP 宿主已经注册，不需要照下方早期示例重复增加模块。下文八工具、语音阶段和各次通过数字均为历史交付记录，不替换为后续全仓结果；当前契约以 `src/mcp/PROTOCOL.md` 和生产代码为准。保留的独立 manifest 仍是复跑入口，本次没有执行 Cargo；新输出不要覆盖旧日志。

## 语音宿主夹具与阶段记录

本夹具直接引用生产 `cli/mcp.rs`、`cli/mcp_voice.rs`、`cli/asr.rs`（真实 BackendArgs）、`toolkit/asr`、`toolkit/audio` 和 `attachment/local_files.rs`，通过 fixture 内模块别名维持原 crate 路径。不复制函数体、不替换本地或云端后端，不引入整个 toolkit 或 WASM 视频运行时。主程序 root 与 McpArgs 由主线程维护。

该阶段过程测试核验 17 项工具注册，保留原来的账号隔离八查询过程和缺账号握手过程。两个新增 Windows 过程使用真实宿主参数：

- 显式云端：凭证文件持有独占句柄，确认不可读取；initialize/tools/list 成功，本机 HTTP 监听器没有收到连接，缓存和运行目录未创建。
- 本地：模型和伪可执行文件均持有独占句柄；initialize/tools/list 成功，不创建媒体输出、临时输出和运行目录。

完整夹具运行同时编译并执行引用模块的原有单元测试；新增握手测试不发送语音请求，不宣称已验证真实转录服务。

Cargo 直接复用相邻 `asr-cache-security/build.rs`，将既有静音/合成 SILK、PCM 和假进程源码逐字节核验后放入本夹具被忽略的 `tests/fixtures/`，满足原测试的仓库相对资源定位。不修改公共 `CARGO_MANIFEST_DIR` 或原测试。

```powershell
$env:LIBCLANG_PATH = 'C:\CodexLocal\build-tools\libclang\clang\native'
cargo test --manifest-path tests/fixtures/mcp-cli/Cargo.toml --lib --test cli-process -- --nocapture
```

2026-09-07 在宿主默认请求级临时目录、错误分类及相对路径修正落盘后复跑：179 项单元测试通过、0 失败，2 项原有 ffmpeg 集成测试保持忽略；4 项过程测试通过、0 失败/忽略，无编译警告。完整输出记录于 `C:\CodexLocal\mcp-cli-voice-fixture-final.log`，前轮记录为 `C:\CodexLocal\mcp-cli-voice-fixture.log`。这不代替后续生产改动后的主仓全量验收。

以下内容保留早期开发记录，不是当前工具数量、McpArgs、模块注册或 transport 字节限制的现行说明；现行生产契约以 `src/mcp/PROTOCOL.md` 和源代码为准。

## 历史记录：初始八工具阶段

## 注册

本worker仅新增src/cli/mcp.rs和本fixture，未修改共享注册/原协议/transport。
由主线程在src/cli/mod.rs完成：

```rust
pub mod mcp;

// Commands enum
/// MCP JSON-RPC stdio（显式 WX_CLI_CONFIG 账号）
Mcp(mcp::McpArgs),

// 命令分发 match
Commands::Mcp(args) => mcp::cmd(args),
```

mcp.rs通过path引用src/mcp/protocol.rs，无需main.rs增加根mcp模块；不要重复注册这个协议源文件。
不要经print_response、emit_warnings、JSON外壳或stdout成功提示。现有cli错误出口只可向stderr打印cmd返回的固定安全错误。

入口：`McpArgs { max_frame_bytes: u32 }`，默认1MiB，范围1024..16777216。
`cmd(McpArgs) -> anyhow::Result<()>`锁定stdin/stdout，直接使用Protocol::serve。
`serve_with(args, BufRead, Write, dispatcher)`用于合成测试或受控嵌入，不包含账号加载。
复用协议类型通过 `cli::mcp::protocol::{Controlled, CallContext, CancellationToken}` 可见，外部嵌入可保留协作式取消边界。

## 启动与账号

MCP客户端进程环境必须显式设置WX_CLI_CONFIG为已有账号配置路径，可继续使用WX_CLI_HOME选择既有运行根。
配置db_dir必须非空明确给出；不调用账号发现、扫描密钥、解密代码，也不读取keys文件。
initialize/ping/tools/list/合法通知不加载配置或发送IPC；首次tools/call才打开该配置并通过RuntimeContext::load取得运行身份。
持有Windows FILE_SHARE_READ配置句柄直到会话结束，允许其他读取、拒绝配置写入/删除替换。编辑/更换配置前退出MCP。
发送前后检查runtime id/config_path/root/db_dir/keys_file/decrypted_dir/wechat_process一致；发现变更后该会话永久拒绝继续查询，避免轮询游标串账号。
这个锁防止普通配置并发写入，不是抵抗有特权本地进程更换目录联接的安全隔离。外部进程不能改变本进程环境；嵌入宿主也不得在运行时改环境或cwd。

发送只调用现有transport::send；它会复用当前账号daemon，必要时沿用已有受控启动流程（该流程的日志在stderr/daemon日志中，非MCP stdout）。本适配不实现新的启动器。
没有ASR/cloud/上传/输出路径参数，也不发送Extract类请求。

## 工具边界

实际使用原协议8tools：get_recent_sessions/get_contacts/get_chat_history/search_messages/decode_transfer/decode_location/get_new_messages/get_chat_images。
get_new_messages使用Sessions的旧式会话摘要快照，不是所有消息增量；get_chat_images仅local_id/时间，明确报告md5/size不可用。
其余9个保持不列出，调用时为未知工具而非空成功：get_contact_tags、get_tag_members、decode_image、decode_file_message、decode_record_item、decode_refer、get_voice_messages、decode_voice、transcribe_voice。
各旧参数/返回差异和所需IPC详见src/mcp/PROTOCOL.md，不因为主线程另接了ASR就自动扩充MCP工具。

## 边界与残留

输入/响应帧长度、LF/CRLF、坏JSON、EOF、stdout仅JSON-RPC全部复用Protocol::serve，不复制实现。
cmd返回的stdio错误不带错误链；工具IPC错误只映射Unavailable，不回显后端message/keys/路径。
协议context默认30秒，callback前后检查，但同步stdin无法在执行send时消费cancelled通知。
真实IPC超时沿用WX_CLI_REQUEST_TIMEOUT_SECS（transport默认300秒）及daemon启动等待期限；推荐MCP部署显式设置该变量为较短期限（如10秒）。30秒是迟到结果拒收边界，不是send的强制中断保证。
进程关闭stdin在帧边界正常退出；阻塞callback期间的EOF/Ctrl+C取决于既有IPC超时/操作系统进程终止，本层不创建不可回收工作线程。
MCP外层有输出上限，但现有transport::request_with_timeout内部read_line仍无字节上限；严格的内部IPC内存预算需要主线程扩展公共transport接口，不能用外层限长冒充已解决。
未来优选公共接口 `send_in_runtime(&RuntimeContext, Request, timeout, max_response_bytes, cancellation)`，届时可移除重复身份检查并传递真实deadline。本文件不调用目前私有request_with_timeout。

## 实跑方法

独立harness引用真实config/runtime/IPC/transport/MCP，但不链接daemon或数据库查询模块。
过程测试创建临时合成账号配置（db_dir/keys路径没有真实数据库或密钥），启动独立命名管道mock；真实transport会发送Ping和八种工具对应查询，由mock按Request返回合成结果。
测试二账号管道隔离、配置写锁、八工具可达、未知工具拒绝、错误脱敏及stdout纯JSON。fixture入口若收到WX_DAEMON_MODE立即退出3，防止意外启动daemon。

```powershell
cargo check --offline --manifest-path tests/fixtures/mcp-cli/Cargo.toml --target x86_64-pc-windows-msvc --target-dir tests/fixtures/mcp-cli/target
cargo test --offline --manifest-path tests/fixtures/mcp-cli/Cargo.toml --target x86_64-pc-windows-msvc --target-dir tests/fixtures/mcp-cli/target --lib mcp::
cargo test --offline --manifest-path tests/fixtures/mcp-cli/Cargo.toml --target x86_64-pc-windows-msvc --target-dir tests/fixtures/mcp-cli/target --test cli-process
```

过滤mcp::只运行本适配及原协议的合成单测，不运行其他模块的系统路径测试。
这不是已注册的真实wx子命令验证；主线程注册后还需在主仓编译及过程验收，不能把独立harness冒称为稳定wx已支持MCP。

本轮实跑：Windows x86_64-pc-windows-msvc，offline check成功；26项适配/协议单测和2项过程测试通过，0失败/忽略。过程测试通过真实transport发送9次Ping和9次查询（八工具加一条合成失败），第二账号收到0次请求；配置写锁生效并在EOF释放，两账号均未创建daemon运行目录。
