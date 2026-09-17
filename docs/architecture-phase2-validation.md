# 消息与媒体切片阶段验收

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

基线为 `def6424`。本记录对应第二阶段，不是整仓重构完成声明。

## 实际接线

| 部分 | 本阶段实现 |
| --- | --- |
| 消息、会话、通话事件 | `business::messages` / `sessions` 定义对象和选择规则；微信适配器读取真实分库、探测能力和解析编码；daemon 查询负责装配与兼容投影。历史、搜索、新消息、通话与会话入口使用实际新实现。 |
| 严格媒体 | 图片与语音关联使用同一消息快照和不透明证据，资源表、语音源与 ASR 数据关联由微信媒体适配器读取。MCP 授权、输出保护和同步语音生命周期保持不变。 |
| 公众号文章 | 已有查询 RPC 与 CLI 接入文章业务模型，适配器负责推送消息与会话未读关联。部分结果和未完成来源显式表达，CLI 保留原数组输出并提示不完整；未新增 HTTP/MCP 入口。 |
| 增量归档 | 直接增量导出与 `export-all --delta-only` 调用同一业务用例，复用固定 runtime 的查询 RPC；已有 writer 使用共享 `ExportTarget` 发布聊天与最后的 manifest。 |

移除了已替代的 query 读取逻辑、ASR 数据关联实现和 daemon operations 的 transport 转发垫片。共享任务 RPC 的真实 transport 没有删除。历史选择旧代码只保留在独立测试夹具中作为冻结参考，不再是生产模块。

## 身份与兼容性

- username 仍在固定账号运行上下文内解释。消息引用绑定一次读取快照，不能序列化为跨读取永久标识；不把 local_id、跨库 rowid 或未经检查的 server_id 宣称为全局身份。
- 全局分页先选择后投影；相同时间的次级顺序不随 newest/oldest 方向翻转，分库排序兼容旧历史行为。普通 JSON 的 source 只规范化斜杠，严格证据保留原始来源。
- 元数据读取与正文能力分离。严格正文查询会先检查所有目标数据流，空的已知表缺正文列也不能被其他分库成功结果掩盖。未知结构、歧义和实际读取超限不会伪装成空成功。
- 新消息仍是原来的时间戳订阅，不宣传为无遗漏历史游标。来源预算是明确的查询限制，不是归档断点。
- 图片严格关联与历史宽松查找保持不同策略；图片发布前再次验证资源证明和清单。语音消息不是通话录音；ASR 只接受三个规范后端名称，云端仍要求显式上传授权。
- 增量原始文档通过明确的 opaque raw-export 边界传递。逐聊天失败记录在 manifest，manifest 发布失败使整次调用失败；既有文件不覆盖，也不把剩余文件当作自动恢复检查点。
- 任务仍由 daemon 排队、执行、取消和持久化。MCP 断连不取消已提交任务，丢失提交响应使用同一幂等键重试；后台重启的 `interrupted` 不表示自动续跑。

## 验证方式

仅使用 Windows x64 MSVC、隔离配置、合成 SQLite/XML 和人工密钥。进程与网络测试使用测试程序和本机合成服务；未访问真实微信账号、扫描微信进程、下载私人媒体或执行云上传。

根测试直接编译真实业务模块，并包含实际 MCP、CLI/Web、daemon 和 worker 链路。独立夹具也引用真实适配器，不以 mock 成功作为通道证明。独立历史夹具的默认测试只是旧选择器参考；带 `runtime` feature 的测试才启动实际 wx 查询进程。

最终验证日期：2026-09-15。历史进程兼容测试曾发现同秒次级排序反转，以及非法参数晚于联系人解析而返回错误类别的问题，均已修改生产实现并增加回归测试，未放宽原断言。同一份真实进程测试已接入根目录 `tests/history_runtime.rs`。

| 检查 | 实际结果 |
| --- | --- |
| `cargo check --offline --locked --target x86_64-pc-windows-msvc` | 退出码 0；仍有 32 条未使用项等编译警告，未用批量禁用警告掩盖。 |
| 根目录 `cargo test --offline --locked --no-fail-fast`，同一 Windows target | 25 个测试目标全部通过，退出码 0；主程序 1188 项通过、10 项忽略。各目标累计 2431 条通过、23 条忽略记录，包含重复编译的测试，不是独立用例数量。 |
| 独立业务模块内存测试 | 21 项通过，不引入具体适配器。 |
| 根目录真实历史进程测试及独立 history `runtime` 目标 | 两种入口均通过，同一份分页、映射、同秒排序及参数校验断言。 |
| 独立 `mcp-readonly-security` 安全目标 | 15 项通过，缺失、损坏、新增来源和空表缺列均拒绝不完整成功。 |
| 其他独立夹具 | 本阶段 image-listing-parity、voice、voice-security、voice-host shutdown、voice-host-security audit、native-image、asr-cache-security、audio、wav-publish、asr-video-security、image-security、native-attachment-security 的指定目标先后通过。没有把未运行的夹具目标计为通过。 |
| 增量导出 | 根测试覆盖真实加密库/RPC、黄金文档、幂等文件命名、保护路径、发布竞争及 manifest 失败；直接业务模块的独立 `rustc --test` 亦通过。旧提取代码式 harness 已改为执行根目录真实检查与测试，脚本语法检查通过，未把未单独执行的脚本当作额外通过结果。 |
| 格式与补丁检查 | `cargo fmt --all -- --check`、`git diff --check` 通过。 |

完整运行日志位于开发机 `C:/CodexLocal/`，最终根检查和测试分别为 `wx-cli-phase2-verified-check.log`、`wx-cli-phase2-verified-test.log`，独立最终检查为 `wx-cli-phase2-readonly-verified.log`、`wx-cli-phase2-history-verified.log`。日志不提交到仓库。前面失败的运行保留原日志，没有替换为成功输出。

## 尚未完成

普通附件查询及聊天目录的全部媒体消费者、表情业务切片、统计与原始批量导出适配、完整归档编排仍需继续迁移。native image 发布尚未完全替换为共享单文件发布核心；CLI transport 垫片仍有调用方。当前不能宣称微信 schema 变化已只影响适配器。

独立 `mcp-voice-host-security` 已使用其 `audit` 目标验证；不带目标的全夹具仍存在导入平台测试所需 Frida 依赖问题，未通过删测试或新增真实扫描来回避。根目录真实进程测试另行覆盖取消与回收。既有按环境条件忽略的测试不计为通过。
