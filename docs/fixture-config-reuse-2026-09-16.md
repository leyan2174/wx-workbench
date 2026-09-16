# MCP 夹具配置复用

## 问题与修改

SNS 适配迁移后的独立图片夹具检查暴露 wx-mcp-cli-harness 依赖的 5 条未使用函数告警：find_config_file、find_existing_config_path、default_config_path、config_path_in_dir、home_config_path。它们在正式工程仍由 RuntimeContext 使用，不是应该删除的正式能力。

CLI 夹具原先复用 mcp-voice-host 的 RuntimeContext，却额外编译同一 config.rs 和 ConfigPin，造成重复配置模块。现在 tests/fixtures/mcp-cli/lib.rs 同时复用宿主公开的 config 模块和 config_pin 模块，与已复用的 RuntimeContext 保持一致。没有新增 allow、虚假调用或扩大正式 API 可见性。

首次只复用 config 模块时，旧重复 ConfigPin 的 load_config_at 调用跨 crate 不可见，检查失败；随后一并复用宿主已有 ConfigPin。失败日志保留为 C:/CodexLocal/wx-workbench-config-fixture-tests.log。未改另一份拥有独立 RuntimeContext 的 mcp-voice-host-security 夹具，避免混用其类型图；共享生成器仍服务该夹具。

## 验证

所有检查使用当前 checkout 专属构建目录 C:/CodexLocal/wx-workbench-target-20260916，离线、锁定依赖、Windows x64 MSVC，不访问真实账号。

- 根工程 cargo check --all-targets 退出 0，日志 C:/CodexLocal/wx-workbench-config-fixture-root-check.log。
- cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/mcp-cli/Cargo.toml --all-targets -- --test-threads=1：24 项模块测试及 4 项真实进程测试通过，无失败/忽略，日志中无 warning；日志 C:/CodexLocal/wx-workbench-config-fixture-tests-2.log。
- 被复用的宿主 ConfigPin 专项：同 target，`--manifest-path tests/fixtures/mcp-voice-host/Cargo.toml --lib config_pin -- --test-threads=1`，2 通过、无失败/忽略，覆盖固定账号身份与操作锁；日志 C:/CodexLocal/wx-workbench-config-pin-reuse-tests.log。
- CLI 夹具严格 all-target Clippy 首次发现共享 tests/fixtures/mcp-auth/build_support.rs 重新分配 Box；改为原位替换语法树块内容，保留原先拒绝夹具启动 daemon 的函数体。随后相同 target、manifest 下 `cargo clippy --offline --locked --all-targets -- -D warnings` 退出 0，日志 C:/CodexLocal/wx-workbench-config-fixture-clippy-2.log。首次错误保留在无 -2 后缀日志。
- 最终夹具矩阵 27 个目标编译通过，所有日志均无默认 warning；日志 C:/CodexLocal/wx-workbench-fixture-warnings-final-2.log。这不是全部夹具运行测试通过。
- 共享生成器另一消费者：`cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/mcp-voice-host-security/Cargo.toml --test audit -- --test-threads=1` 为 19 通过、1 项原有符号链接权限忽略，无失败，日志 C:/CodexLocal/wx-workbench-config-fixture-host-audit.log。错误地把仅有共享源码的 mcp-auth 目录当独立 crate 的一次调用未运行任何测试，不计通过。

这是测试装配重复清理，不改变生产控制流、业务对象、对外命令或存储；没有用户迁移要求。原配置/固定行为单测仍在正式源码与宿主目标中，并未删除断言。此清理不是复杂度热点修复；同期额外规则结果见[表情适配迁移](emoticon-format-adapter-2026-09-16.md)。

## 联系人夹具后续修正

contact-rows 中仅供单测的响应投影注册加 cfg(test)，不改生产函数、不加 allow。其运行测试还发现旧断言要求空表返回成功空列表，与当前内存 Directory、SQLite 适配器及 daemon 空缓存拒绝契约不符；首次 25 通过、1 失败，日志 C:/CodexLocal/wx-workbench-contact-fixture-scope-tests.log。

用例已同步为精确的 typed Unavailable 及源文件不变断言，同时验证有效源筛选无匹配仍返回空列表，没有修改生产行为。旧成功断言不再是正式契约。复测 `cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/contact-rows/Cargo.toml -- --test-threads=1`：26 通过，无失败/忽略，日志同前缀 tests-2.log。
