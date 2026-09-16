# SNS 媒体适配器迁移

## 基线与范围

工作区为 wx-workbench，main 分支；本轮叠加在尚未提交的重构工作上，不重置或提交这些修改。迁移前共享密钥流已完成 VideoRuntime 到 SnsKeystream 的命名调整，本轮只修正实现归属及调用方向，不再次实现密码算法。

迁移前验证：SNS 模块 97 通过、2 项原有忽略；视频安全集成目标 366 通过、3 项原有忽略；相册集成目标 12 通过。严格 all-target Clippy、格式及独立夹具编译矩阵均通过。测试目标包含共享源码，不应将这些数字当成不重复的场景总数。

## 已实施

- 将 `toolkit/sns/keystream.rs` 和相应测试移到 `adapters/wechat/media/sns_keystream.rs`、`sns_keystream_tests.rs`。该代码处理微信 WxIsaac64 WASM 的固定 ABI、内存与执行预算、密钥流及视频前缀恢复，属于微信格式适配，不属于相册编排。
- 相册图片、视频和单视频执行宿主直接引用适配器；移除旧模块注册，不增加生产转发层。业务层没有反向依赖该适配器。
- 原独立向量和安全夹具编译同一份迁移后的源码；图片/视频夹具只调整测试模块装配，不复制算法。迁移时本地审计 WASM 和合成向量路径随移动修正。
- WASM 契约移为 `adapters/wechat/media/SNS_KEYSTREAM.md`，更新当前架构、迁移表和使用说明链接。历史报告不改写为最新状态。

## 保留的契约

仍使用原第三方 WASM、固定摘要和来源说明，不重新署名；不是 AES 或音视频 codec。保持每次调用独立 Store、错误脱敏、内存清零、输入/燃料/内存预算。图片和视频共用密钥流；视频仅恢复前 128 KiB，尾部原样保留。格式标记不等于认证，不能据此保证检测所有错误密钥。

网络、账号授权、文件路径保护和原子发布仍由现有工作流及执行边界负责。没有改公开命令、配置、存储格式、获取算法或恢复策略，不要求用户重新初始化。本轮移除的是内部模块入口，不删除任何用户文件或支持的媒体格式。

控制流与函数签名未变，未通过拆函数降低复杂度。本轮默认严格告警结果与额外认知复杂度应分开报告；额外规则尚未重跑，不能声称之前四个热点已归零。

## 验证记录

使用 `C:/CodexLocal/wx-workbench-target-20260916`，LIBCLANG_PATH 指向现有 clang/native。未访问真实账号或私人媒体。

- `cargo check --offline --locked --target x86_64-pc-windows-msvc --all-targets` 退出 0；日志 `C:/CodexLocal/wx-workbench-sns-adapter-check.log`。
- 根工程全量命令 `cargo test --offline --locked --target x86_64-pc-windows-msvc --all-targets --no-fail-fast -- --test-threads=1` 退出 0；33 个目标合计 2974 通过、0 失败、23 项原有忽略，未扩大忽略范围。其中主目标 1416 通过、11 忽略；不同目标重复包含共享单测，合计不是独立场景数量。日志 `C:/CodexLocal/wx-workbench-sns-adapter-full-test.log`。这是本轮源码的全量证据，不表示尚未实现的架构目标已经完成。
- 迁移后 `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 和 `cargo fmt --all -- --check` 均退出 0，日志分别为 `C:/CodexLocal/wx-workbench-sns-adapter-clippy.log`、`C:/CodexLocal/wx-workbench-sns-adapter-fmt.log`。
- 独立密钥流夹具：`cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/sns-video-native/Cargo.toml -- --test-threads=1` 为 9 通过、0 失败/忽略，日志 `C:/CodexLocal/wx-workbench-sns-adapter-isolated.log`。该夹具仅依赖 wasmi、sha2、zeroize 和测试向量 JSON，不依赖 SQLite、daemon 或 toolkit；覆盖逐字节向量、未知资产、非法密钥、预算耗尽和调用隔离，而非仅做路径扫描。
- 首轮 `scripts/check-fixtures.ps1 -Offline` 在 mcp-image-security 失败：该夹具直接嵌入完整媒体适配模块，却未声明 wasmi。已给该夹具添加与根工程相同的精确版本和 feature，更新锁文件，不屏蔽模块。首次日志 `C:/CodexLocal/wx-workbench-sns-adapter-fixtures.log`；修复后整套矩阵退出 0，日志 `C:/CodexLocal/wx-workbench-sns-adapter-fixtures-2.log`。矩阵是编译检查，不是测试执行。
- 补跑 `cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/mcp-image-security/Cargo.toml --test audit -- --test-threads=1`：20 通过、0 失败、2 项原有 Windows 符号链接权限忽略。日志 `C:/CodexLocal/wx-workbench-sns-adapter-image-audit.log`。补充变更仅在独立夹具清单及锁文件，根工程源码未在全量回归之后改变。
- 独立图片夹具的依赖 wx-mcp-cli-harness 仍报告 5 个 config 辅助函数未使用告警（find_config_file、find_existing_config_path、default_config_path、config_path_in_dir、home_config_path）；它们与根工程严格 Clippy 检查的可达范围不同，本轮未抑制或删除。不能将根工程零告警表述为所有独立依赖均零告警。

## 未完成

### 公开候选的后续收口

迁移后发现固定 WASM 的来源副本没有附带可验证的再分发许可证。公开默认构建现已停止嵌入、下载或自动发现该资产：`SnsKeystream::bundled` 明确返回资产不可用；单视频加密解码只能通过 `--wasm` 使用调用方已获授权且哈希匹配的本地文件。明文 MP4 仍可直通。相册入口当前没有外部 WASM 参数，需要该密钥流的远端加密媒体会报告引擎不可用；不能把这一限制描述成完整朋友圈媒体支持。

`sns-wasm-test-asset` 只为持有合法本地审计副本的内部验证保留，不能随公开源码或二进制候选启用。最终公开候选仍须在资产物理缺席时完成干净构建与默认测试；本节记录的是代码策略，不代替该最终证据。

后续已通过[配置夹具复用](fixture-config-reuse-2026-09-16.md)消除上节记录的 5 条依赖告警；27 个夹具最新编译日志无默认 warning。远端表情纯格式也已迁入既有表情适配器，详见[独立报告](emoticon-format-adapter-2026-09-16.md)。以下其他残余项仍未完成。

本轮只完成 SNS 密钥流实现归属，不能据此声称所有媒体业务已分层。DAT 和远端表情格式归属、剩余密钥读取路径、ASR 惰性材料交付、大清单通信仍需推进。数据库材料当前仍嵌入 worker 初始载荷，64 KiB 请求预算与最多 4096 项材料的容量不匹配问题没有在本轮解决。
