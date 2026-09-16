# 编译告警维护

本轮针对 `f85ca4c` 的 Windows x64 MSVC 编译告警进行清理，不改变命令参数、wire 字段、账号绑定或授权语义。

## 处理原则

- 删除已无调用方的迁移残留：旧缓存构造、联系人 MD5 反查缓存、严格消息包装、位置摘要包装及无用 getter。同步清理构造处，不增加假引用来“使用”字段。
- 仅供单测使用的便利入口、常量和 re-export 使用 `cfg(test)`。正式代码继续使用固定上下文和受检入口；测试夹具不改用成功 stub。
- RawMessage 不再携带无人消费的 sender_id/sort_seq 副本；发送者解析和 sort_seq 值类型/大小校验保留，并有合成回归覆盖。原始导出的显式投影不变。
- 必须保留的业务契约成员使用逐成员、带理由的 `expect(dead_code)`，例如未知媒体类别、阶段性失败、来源完整性和分页结束状态。不会将这些状态删除或混同为成功空值。测试中已消费的成员只在非测试构建启用预期。
- `expect` 不是永久忽略：后续生产调用使预期不再成立时，编译器会报告 `unfulfilled_lint_expectations`，应移除相应标注。
- 精简夹具嵌入公开生产模块时，其可达性与正式二进制不同。仅在具体模块引入点对相应 dead_code/预期差异作说明，不关闭整个程序的 warnings。共享测试助手只对确实由部分目标使用的成员说明例外。详见 [夹具告警归属](../tests/support/WARNINGS.md)。

## 检查范围

根工程使用 `cargo check --offline --locked --all-targets --target x86_64-pc-windows-msvc`，覆盖两个二进制及根工程测试目标。

独立夹具应按 Cargo 清单声明的目标检查。`--all-targets` 会额外强制编译标记 `test = false` 的库或探针单测；只读安全、插桩图片和 WAV 夹具通过其独立集成测试验证，不能将复制整个应用或丢弃现有断言作为告警处理办法。

本轮只集中执行受影响测试，不因消除一条告警而重跑所有无关测试。日志保存在仓库外，不提交构建缓存、配置、密钥或私人数据。

## 本轮编译结果

根工程全部目标、26 个独立夹具的有效目标均已验证零错误、零告警。完整目标选择见 [检查矩阵](../tests/support/CHECK_MATRIX.md)，两个 `runtime` feature 也已启用检查。语音夹具现在只嵌入实际使用的共享 managed 进程模块，保留其库单测，不再误引 Frida 管道测试。

- 基线：`C:/CodexLocal/wx-cli-warnings-before.jsonl`，两个正式二进制各有 40 条告警，另有测试构建告警。
- 根工程及 23 个独立夹具：`C:/CodexLocal/wx-cli-warning-final2-<name>.jsonl`，其中根工程 name 为 `root`。
- 其余三个：`C:/CodexLocal/wx-cli-warning-fix-{mcp-history-compat,plan-selection,native-attachment-security}.jsonl`，均零诊断。前两者设置编译期 `CARGO_BIN_EXE_wx` 为同次构建目录的真实 wx 程序；后者恢复测试专用媒体模块路径。
- 首次强制夹具 `--all-targets` 的失败、批处理参数错误以及 final2 中的三个失败均保留原日志，不改写为一次全矩阵通过。结果同时检查 rustc JSON 和 Cargo 普通 warning/error 行。

根工程定向测试已通过 556 项，4 项忽略：两项需本机 FFmpeg/ffprobe 的音频集成、一项需 Windows 符号链接权限、一项隔离代理子进程助手。忽略项不计通过。覆盖业务/架构契约、适配器、daemon 查询及缓存、音频、SNS、IPC 和位置投影，没有重跑无关的整仓测试。相关日志前缀为 `C:/CodexLocal/wx-cli-warning-tests-`。

受影响夹具另通过 83 项：下载监督 3、语音宿主关闭 1、语音宿主安全 19、附件 58、历史 runtime 1、计划选择 runtime 1；语音安全另有一项符号链接权限用例忽略。合计 639 项通过、5 项忽略。夹具日志前缀为 `C:/CodexLocal/wx-cli-warning-runtime-`。

计划选择 runtime 当时首次失败是夹具在受保护存储初始化之后新增 session 库密钥，却没有更新测试存储。修复后，原有选择/日期/增量/只读断言全部通过；未放宽生产读取限制。复测日志为 `C:/CodexLocal/wx-cli-warning-plan-fix-test.log`，零告警编译记录为同前缀 `-check.log`。当时使用的旧迁移 helper 现已退役，当前夹具通过 `Account::seed_keys` 直接构造正式 DPAPI 测试存储；此处日志是历史验证记录，不代表本轮已执行测试。

最终根工程 `--all-targets` 检查 exit 0、warnings 0，日志为 `C:/CodexLocal/wx-cli-warning-delivery-check.log`。以上是集中定向验证，不宣称重新执行了全部无关测试。没有访问真实账号、执行真实内存扫描或真实云上传。
