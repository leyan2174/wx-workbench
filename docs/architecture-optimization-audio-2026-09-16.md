# 音频边界优化报告

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

日期：2026-09-16

## 当前基线与检查范围

本轮基于当前脏工作区继续迁移 `src/toolkit`，不重置或覆盖其他改动。支持目标为 Windows x64 MSVC，构建使用本 checkout 专属 `C:\CodexLocal\wx-workbench-target-20260916`。检查范围为原 `toolkit/audio`、其 daemon/ASR/MCP 调用方、相关 fixture 和当前架构文档；全部测试使用合成音频、临时 SQLite 与隔离输出，未访问真实账号、密钥、聊天数据或外部转录服务。

## 已发现并修复

1. 原 `toolkit/audio` 同时拥有格式基础设施和数据库批次应用编排，模块名不能表达责任。
2. WAV 发布依赖 `toolkit::asr::validate_wav`，形成基础设施反向依赖转录工作流。
3. `mcp-image-security` 的 build script 从 ASR 源码抽取 WAV parser AST，形成第二份测试实现。
4. daemon 前台 operation、任务 worker、MCP voice、ASR 和聊天目录通过 Toolkit 间接访问音频能力。

实际修改：

- SILK 校验/解码、PCM/WAV 转换、受控 ffmpeg、MP3/WAV 发布迁至 `infrastructure::audio`。
- 数据库语音批次编排迁至 `application::voice_batch_export`；微信 SQLite 读取继续复用 `adapters::wechat::media::voice_export`，业务计数继续复用 `business::voice_export`。
- WAV parser 独立为 `infrastructure/audio/wav.rs`，ASR 改为消费者。
- 所有生产调用方直接依赖真实所有者，删除 Toolkit audio 注册，不保留转发层。
- 独立 fixture 改为编译真实 audio/wav 源码，删除 WAV parser AST 抽取。
- 同步入口文档、架构说明、迁移表、fixture 脚本与测试路径。

## 边界与复杂度指标

| 指标 | 修改前 | 修改后 |
| --- | ---: | ---: |
| `src/toolkit` 文件数 | 53 | 44 |
| Toolkit 外部生产依赖文件数 | 19 | 17 |
| 音频生产所有者 | 1 个混合 Toolkit 域 | 1 个 infrastructure 域 + 1 个 application 用例 |
| WAV 发布到格式校验的跨层反向依赖 | 1 | 0 |
| fixture 中 AST 抽取的 WAV parser | 1 | 0 |
| 旧 Toolkit audio 生产/测试/脚本引用 | 多处 | 0 |

本轮没有为降低指标机械拆分编码函数，也没有引入注册表、回调框架或新依赖。复杂度改善来自所有权和依赖方向收敛；未声称算法圈复杂度发生变化。

## 验证结果

- Windows MSVC `cargo check --all-targets`：通过。
- Windows MSVC `cargo clippy --all-targets -- -D warnings`：通过，默认告警和 Clippy 告警均为 0。
- `infrastructure::audio`：14 项通过，1 项因未显式要求系统 ffmpeg/ffprobe 而忽略。一次与大型测试并发运行时输出洪泛用例超时，随后单独精确重跑通过。
- `application::voice_batch_export`：16 项通过，1 项同因忽略。
- ASR pipeline：11 项通过。
- `asr_video_security`：340 项通过，2 项既有进程夹具忽略。
- `mcp_audio_adapter`：186 项通过。
- 独立 `wav-publish`：库测试 198 项通过、2 项既有条件忽略；发布契约 1 项通过。符号链接场景因当前 Windows 权限报告 `UNVERIFIED`。
- `cargo fmt --check` 与 `git diff --check`：通过；后者仅报告工作区既有 LF/CRLF 转换提示。

## 未验证与保留问题

- 本切片未运行根工程完整 `cargo test`，不能据此宣称全仓最终验收完成。
- 未运行需要真实 ffmpeg/ffprobe 的两个显式忽略用例。
- Windows 当前账户缺少创建符号链接权限，对应 WAV 发布竞争场景未验证。
- 本切片结束时 `src/toolkit` 仍有 ASR 与 SNS 两个真实工作流域；后续切片已完成调用方、测试与文档迁移并物理删除该目录，当前状态以[架构说明](architecture.md)为准。

以上未验证项没有被记为通过。本轮未提交、推送或发布。
