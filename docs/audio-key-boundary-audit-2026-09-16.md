# 音频格式与密码材料审计

日期：2026-09-16。基于当前未提交工作区，属于当前实现审计，不改写第三方历史。

## 字段与消费者

| 位置 | 实际含义及消费者 | 处理 |
| --- | --- | --- |
| `key_store::Record.account_key` | 可选账号材料；scanner 已有 provider 可返回或验证材料 | 保留，不推断服务端保存或通用派生关系 |
| `Record.database_keys` | 数据库相对路径到密码材料；DbCache 解密 SQLite | 保留，语音数据库也在该层 |
| `Record.image_key` | 16 字节 AES 与 XOR；图片解密实际使用 | 保留，不改叫 codec |
| `service::worker_keys::MaterialChange` | Account、Databases、Image 三种变更 | 无音频参数，未改 wire format |
| `worker_keys::Access` | 账号绑定、权限、版本和相应秘密快照 | 无语音秘密或编解码选项 |
| `voice_export::media_keys` | 数据库路径清单，不包含密钥字节 | 改为 `media_database_paths`，同步导出和测试 |
| ASR `batch/files::voice_key` | 来源与 local_id 的匹配身份 | 改为 `voice_identity`，不改变去重和恢复语义 |
| `audio::decode_silk_to_pcm` | 仅接受音频字节；SILK SDK 输出固定 24 kHz、单声道、16 位 PCM | 保留，不添加密码或配置层 |
| Config、service 请求、MCP 语音调用 | 未发现 voice_key/audio_key 或音频参数进入秘密链路 | 新增 MCP 未知参数拒绝断言 |

查询中的媒体路径局部变量也改为 `media_paths`。这些都是内部名称，不修改公开字段或磁盘版本。未发现用户推测的“语音参数保存在密钥库”实现，不能声称删除了不存在的秘密字段或迁移过用户材料。

## 图片文件恢复的后续整改

经生产调用复核，DAT 处理包含 AES/XOR 恢复与标准文件头识别，并不将 JPEG/PNG 解码成像素。内部 `DecodedImage` 改为 `RestoredImage`，统一入口 `dispatch` 与 V1/V2 的 `decode` 改为 `restore`；图片提取、批处理、聊天目录、SNS 归档、离线材料验证及样本导出调用同步，不保留旧名称转发层。`decoder` 响应字段和缓存目录名不变，恢复后的文件字节不强制重新编码。不同于 SILK 到 PCM 的真实 codec 解码。

另发现 `V2KeyMaterial` 自动派生 Debug 会打印 AES 字节，已替换为不区分有无材料的固定脱敏输出并增加断言。没有改动 AES/XOR 算法、实际材料来源、DPAPI 格式或公开请求参数。SNS URL 内容恢复与远程表情转码尚未在本小节宣称统一完成。

## 验证范围（语音切片）

使用人工 DPAPI 材料、合成 SQLCipher 数据库和仓库合成 SILK 音频，不扫描真实进程、不读取私人数据、不调用外部转录。

- 新增 Record 反例：合法数据库材料可读取，加入 voice_key、audio_key、codec、sample_rate、channels 任一额外字段则拒绝整个记录。
- 现有密钥测试复跑包括账号隔离、损坏拒绝、图片材料和原子更新；12 项通过。
- 真实 MCP 解码专项通过：从合成加密库读取音频、验证精确 WAV 与不同账号输出、保留已有文件；新增 voice_key、codec、sample_rate 参数均以协议错误拒绝。
- 解码器不接受密钥或 RuntimeContext；合成 SDK 对照测试及未知/损坏容器测试单独运行。没有为了验证“不读取 DPAPI”引入新的模拟密钥服务。

实际命令均使用 `--offline --locked --target x86_64-pc-windows-msvc` 和本 checkout 专属 target：

| 命令（省略上述公共选项） | 结果 | 日志后缀 |
| --- | --- | --- |
| `cargo check --all-targets` | 通过 | `check.log` |
| `cargo test --bin wx key_store -- --test-threads=1` | 12 通过 | `keys.log` |
| `cargo test --test mcp_voice_runtime real_voice_media_ids_decode_exact_wav_and_preserve_session_and_accounts -- --test-threads=1` | 1 通过，6 未选择 | `mcp.log` |
| `cargo test --bin wx toolkit::audio::tests -- --test-threads=1` | 7 通过，1 原有 FFmpeg 条件集成测试忽略 | `toolkit-audio-tests.log` |
| `cargo test --bin wx adapters::wechat::media::voice_export -- --test-threads=1` | 4 通过 | `adapters-wechat-media-voice_export.log` |
| `cargo test --bin wx identity_tests -- --test-threads=1` | 10 通过，其中新增 ASR 身份合并反例 2 项 | `identity.log` |

早先 `toolkit::asr::batch::files` 和 `toolkit::asr::batch` 过滤各运行零项，未计为覆盖。之后新增纯内存用例验证重复 local_id 按来源区分，以及重复身份或不同 username 时不部分合并。默认严格 all-target Clippy 与格式检查通过；没有重新运行额外复杂度规则、独立 fixture 矩阵或根工程全量。

检查日志前缀为 `C:/CodexLocal/wx-workbench-audio-boundary-`。这是专项审计，不是整仓最终验收。尚未验证真实微信版本的密钥产生与派生机制，也未改变现有 provider 算法。ASR 批处理读取数据库材料的 daemon 快照迁移属于另一个待完成边界。

新增测试后的严格 Clippy 曾报告 `items_after_test_module`（`clippy-2.log`），已将该模块移至文件末尾，未添加 allow。更广泛的媒体模型/处理体系整改按最新执行顺序暂缓，本轮未据此新增模型或扩大生产流程修改。
