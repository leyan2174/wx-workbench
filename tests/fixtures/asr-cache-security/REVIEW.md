# 审查结论

> 历史审查与修复时间线：各节的“最新”“当前”只指对应记录时点，测试数字、失败和忽略原因不改写为后续主仓结果。2026-09-07 本次仅同步说明；现行复跑入口及旧日志保护要求见 [README.md](README.md)。

## 历史最终记录：无过滤路径修复与硬链接复验通过

专属 build.rs 自动准备 11 个既有合成夹具（假后端源码及五组 SILK/PCM），解决被引用原测试的 CARGO_MANIFEST_DIR 相对路径错误；生成文件仅在本 fixture 被忽略的 tests/fixtures 目录内。未改生产、公共 root 或其他测试，也未恢复已删除的 store_result/FailureSkipped。

无过滤 `cargo test --offline --manifest-path tests/fixtures/asr-cache-security/Cargo.toml --target-dir C:\CodexLocal\build\asr-cache-security -- --nocapture` 已完成：**137 passed、0 failed、3 ignored、0 filtered out**，无编译警告。完整命令与输出见 unfiltered-path-fixed.log。此前 12 项缺路径失败消失；三个忽略项是原有两个 ffmpeg 集成要求和本 fixture 的 Windows 文件符号链接权限限制。

运行前收到 Poincare 生产硬链接修复通知，随后独立执行原 `cli_external_database_hardlink_must_be_rejected_at_path_preflight`，未修改或弱化断言。实际错误已变为 `cache aliases a database input`，源与别名原字节保持不变，两项专属 CLI 路径测试均通过，原 P2 发现据此关闭。新 source_files 复用非递归、受限的 shard_names，保护消息/媒体及固定可选联系人数据库；目录 junction、源间别名、4097 条目限额等真实内嵌测试也在此次无过滤运行中通过。

以下均为历史记录，红测试/待修复状态已由本节取代；可信稳定目录和非 CAS 的既定限制仍不变。

## WouldBlock 修复后复跑

Laplace 修复后的 cached_tests.rs（SHA256 `19FC04771707A12316B126C6E622198DDD2C41C954B7634DC7FC8405084B8FA0`）已独立复跑：核心 8、安全 8、缓存适配 7、新增适配 5，共 28 通过、1 文件符号链接权限忽略，无编译警告，未再出现 Windows 10035。完整命令/输出见 wouldblock-fixed-run.log，全部引用源在四组测试前后哈希一致。

数据库文件外部硬链接预检问题仍待 Poincare 修复后重跑原断言；本轮没有修改或弱化该失败测试，也没有改生产源码。以下旧记录中的“等待 WouldBlock 修复”状态已被本节取代，硬链接发现尚未关闭。

## 最新交付：CLI 路径审查

### [P2] 数据库文件在目录外的硬链接未被缓存路径预检拒绝

位置：`src/cli/asr_database.rs:151`。数据库输入只做目录路径分离；后面的 `same_file` 身份检查仅覆盖后端程序、模型、密钥。因而 `db/message/message_0.db` 的外部硬链接 `cache.json` 能通过预检。本专属测试随后得到 `MissingDatabase: no media shards`，证明已进入数据库解析，而不是别名拒绝。

复现：`cli_path_tests::cli_external_database_hardlink_must_be_rejected_at_path_preflight`，完整输出见 cli-path-rerun.log。它只创建临时合成 sentinel 和硬链接，无有效账号、模型或真实数据库；源与别名原字节保持不变。影响限定为不满足本轮要求的源别名拒绝，**没有证明数据损坏、凭据泄漏或缓存写穿硬链接**。该问题不依赖父目录竞态或 non-CAS。建议在已有缓存路径存在时，对显式数据库源清单比较文件身份；无需修改可信稳定父目录的既有前提。

### 验证状态

- 最新核心/安全/薄适配/新增适配四组分别为 8、8、7、5 通过，共 28 通过、1 文件符号链接权限忽略，无编译警告；测试前后源哈希一致。见 latest-review-run.log 与 source-hashes.log。
- 新 CLI 窄测试 1 通过、1 上述别名预检失败；通过项实际验证源目录内路径、程序、模型、密钥硬链接拒绝及原文件保持不变。真实文件隔离逻辑以 path 引用 `src/toolkit/files.rs` 并 re-export `toolkit::separate`，未使用替身实现。
- 原先缺 accessor 的旧断言已被 Laplace 更新，本轮已通过 7 项真实 cached 原测试。云端身份含非秘密配置，不含认证头；账号隔离、字节限额、损坏不覆盖、错误不持久化没有新实质发现。
- MAIN 后续报告全量云回环测试出现 Windows 10035（非阻塞 accepted socket 继承行为），由 Laplace 修测试。独立 latest-review-run.log 此次未遇到该错误；不将其归类为生产漏洞，不调整 timeout，也不以独立通过声称全量已绿。修复后的回环复跑结果尚不在本报告中。

仅修改本专属 fixture，未改公共 root、CLI、生产缓存或其他测试。以下保留历史检查记录，若与本节冲突，以本节及其日志为准。

## 注册后的续审（最新状态）

本审查从未修改公共 `asr/mod.rs`。公共注册出现后，仅调整专属 lib.rs 为 re-export 已注册的 cache/cached，避免编译两份类型；没有撤销或改动公共注册。

当前四组复跑在原 `cached_tests.rs:69` 停止：云端身份实现已新增 `client.cache_identity()`，旧测试仍要求 `accessor required` 错误。复现见 registered-review-run.log，属于实现变化后的旧测试判据不一致，不据此声称上传授权绕过。公共测试未由本审查修改。

随后新增适配攻击测试独立跑完 5 项通过（adapter-continued.log）：坏字段/时间不符的缓存命中不得作为旧结果返回，也不覆盖原记录；已有缓存不得绕过音频格式或 16 MiB 限额。核心两组仍为 8+8 通过、1 权限忽略；原薄适配组为 5 通过、1 旧断言失败。当前不具备全部通过结论，也不沿用下文历史版本的云端身份缺失限制。

以下为上一轮、云端 identity accessor 尚未添加时的审查记录。

当前所审源与日志见 source-hashes.log。未发现违反已声明保证的实质缺陷；这不是跨平台、真实模型、整仓集成或并发线性一致性认证。

## 实际证据

- 核心原测试 8 项、新增核心安全测试 8 项、薄适配原测试 6 项、新增适配攻击测试 3 项通过，共 25 项；文件符号链接 1 项因当前宿主 Windows 1314 明确忽略。
- 失败携带的合成凭据不创建/修改缓存；账号、音频、配置身份明文不落盘。API 不接收凭据字段，但调用方仍须避免把凭据放入转录正文等任意字符串，不声称自动识别秘密。
- 打开缓存之后替换为坏 JSON、错误账号、非封套格式，lookup/store 均拒绝且原字节不变。未知封套/根/条目字段及大整数、高精度小数在新增记录后保持值不变。
- 超过 64 MiB 的无效 JSON 优先返回读取大小错误；真实代码以 Read::take(limit+1) 限制输入再解析。不是先无界 read 后检查，也不声称 serde DOM 或进程内存低于 64 MiB。
- 硬链接发布替换的是缓存目录项，别名文件原字节不变；最终目录 junction 被拒绝，指向的测试目录不变。文件符号链接的实际权限覆盖仍缺失，见初次 test.log。
- 原核心测试保留发布前字节/身份冲突检测；不将最后一次检查后非协作替换窗口作为新缺陷。可信稳定父目录是明确前提，不声称阻挡父目录改名、恶意 junction 更换或权限对抗。
- 真实薄适配原测试验证音频、模型内容、可执行程序内容变化不能错误命中；空成功能命中。新增假后端尝试写已持有的模型句柄被 Windows 分享模式拒绝；外部正常换模型后不能沿用旧成功。
- 未授权云端请求在无效账号、音频和坏缓存同时存在时优先得到授权错误。源审查确认授权检查位于文件访问和音频校验之前；动态用例不是操作系统级所有读取调用的追踪。
- 后端失败诊断不进入返回错误/缓存；坏缓存不被覆写，成功识别返回 ReadUnavailable；锁冲突返回 WriteUnavailable 且不丢失转录成功。

## 已知限制

当前 ExplicitOpenAi 缓存身份分支明确报 non-secret backend accessor unavailable，未实现云端缓存命中，不作为新的安全 bug；本审查没有真实网络、账号或模型验证。Cache 的 account_sha256 是命名隔离而非认证签名，调用方提供正确身份是前提。缓存目录及转录正文访问权限仍由调用方负责。

初次环境失败与 harness 缺引用的编译日志保留，最终结论以 core.log、security-final.log、cached.log、adapter.log 为准。run.ps1 对前后源哈希进行比较，源变化时拒绝将混合版本测试当成同一版本验收。
