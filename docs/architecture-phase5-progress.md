# 架构第五阶段进展

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

基线为已推送的 `94c8441b4c2419eab5089498cefcecd974d49cab`。本记录描述开发中的工作区，不是最终验收或测试通过声明。

按用户调整后的节奏，先完成剩余实现并编写测试，保留必要的编译检查；所有实现稳定后统一执行专项测试、全量回归及最终审查，不再每个切片重跑全量。

## 已接线的库存与诊断

- `adapters/wechat/messages/inventory.rs` 拥有普通消息文件识别、大小写与路径分隔符归一化及未知来源排序。`daemon/meta.rs` 仅委托发现，并保留历史提示接口的容错行为；完整性查询仍走 checked 路径，读取失败不能变成完整结果。
- `adapters/wechat/messages/probe.rs` 拥有会话数据源定位与有界只读探测，返回读取数量和最新时间戳，不返回数据库行。`DbCache` 继续负责真实缓存解析、解密和计时；没有新增预热、失效或任意 SQL 入口。
- 目录原始导出的未映射身份检查复用消息适配器的规范表名校验。宿主中已无调用的旧正则删除，未知目录占位名称作为旧原始协议投影保留，不升级为业务账号身份。
- 独立夹具引用相同的生产库存和探测模块；新增合成数据库测试覆盖上限、重复时间戳、空结果、未知字段、错误类型及只读不创建文件。原有库存错误与大小写测试继续验证真实适配器。
- 未映射会话改为 `UnmappedConversation` 不透明引用。旧查询和原始目录协议通过适配器诊断接口取得兼容字段；业务代码不直接获得表哈希，也不将未映射来源当成稳定联系人身份。

## 已接线的媒体、引用与筛选

- 附件内容、媒体类型编码、缓存目录规则和旧 DAT 定位分别由 `adapters/wechat/media` 的专门模块拥有。`business/attachment_content.rs` 表达解码后的附件内容，宿主继续负责授权、文件访问和发布。严格规则不自动回退到旧启发式路径。
- 严格引用回复由消息适配器解析成业务 `Reply`，入口保留旧 JSON 投影。`sender_label` 明确表示预览标签，不冒充稳定身份；来源中的作者证据仍可通过显式诊断字段保留。
- 转写准备与回执复用消息适配器的来源证明校验。摘要、账号绑定、回执生命周期以及两条路径已有的严格度差异未合并。
- CLI 与 MCP 使用业务 `FilterLabel` 和 `service/message_filter.rs` 的旧协议投影。微信适配器独自解释存储过滤规则；旧 Video=43 不扩展为 62，System=10000 不扩展为 10002，Link/File/App 的宽筛选及打包数值精确匹配保持兼容。CLI 与 MCP 的允许标签、大小写和别名差异由入口策略保留。

上述实现及新增测试已落地，集成编译通过；运行结果仍待统一测试完成。

## 其余迁移与审查

结构化消息的转账、位置和阅读摘要解析已迁入消息适配器，原始导出正文转换也改由适配器拥有；展示模块保留语义模型到原有文案、JSON 的投影。转账业务状态使用枚举，原始子类型仅作为来源证据，不再代替业务状态。除第四阶段列出的七项外，继续复核发现的 CLI/MCP 类型映射重复、展示模块的转账和导出内容 XML 解析均纳入本轮实现，不把最初的七项清单当成整仓验收范围。

各切片必须保留既有 wire format、严格与兼容策略、来源证据以及授权边界。私有格式解析迁入适配器不表示远端历史完整、未知通话媒介可推断、媒体启发式关联变成严格关联，或后台 interrupted 任务能够自动续跑。

## 验证状态

第四阶段的通过记录只证明第四阶段提交。第五阶段新增测试代码不等于测试已通过。全部使用合成材料，不访问真实账号、微信进程、私人媒体或云上传。

本机日志保存在 `C:/CodexLocal/`，不随源码提交：

| 检查 | 结果 | 日志 |
| --- | --- | --- |
| 集成编译 1 | 通过，34 条警告 | `wx-cli-phase5-check-1.log` |
| 统一根测试 1 | 构建失败，位置消息兼容测试缺少旧字段清单，尚未运行测试 | `wx-cli-final-root-tests-1.log` |
| 集成编译 2 | 通过，34 条警告 | `wx-cli-phase5-check-2.log` |
| 统一根测试 2 | 已结束，25 个目标、2713 条通过记录、2 条失败、23 条忽略；失败均为合成 HTTP 服务端 WouldBlock | `wx-cli-final-root-tests-2.log` |
| 修正测试服务端后的编译 | 通过 | `wx-cli-final-check-3.log` |
| 表情下载单元重测 | 16 通过、1 忽略，零失败 | `wx-cli-final-emoticons-unit.log` |
| 表情运行时重测 | 6 通过，零失败 | `wx-cli-final-emoticons-runtime.log` |
| 最终集成编译 | 通过，34 条警告 | `wx-cli-final-check-5.log` |
| 库存与元数据重测 | 12 通过，零失败 | `wx-cli-final-inventory-tests.log` |
| 业务与架构约束重测 | 纯业务 36、公共契约 3、入口约束 4，通过且零失败 | `wx-cli-final-contract-tests.log` |

位置消息测试改为独立列举旧协议字段，未暴露适配器私有常量，原有输出内容、字段数量、坐标和异常值断言全部保留。

第二轮失败后只调整两个合成服务端的已接受连接：显式切回阻塞模式，并设置读写超时。生产代码及下载、禁止覆盖、精确输出字节断言均未改变。重测覆盖两个失败用例及相邻下载用例，不重新累计未改动目标的通过记录，也不将原始全量命令的退出码改写为成功。

## 独立夹具结果

以下 16 组均退出成功。数字是通过记录，不是跨目标去重后的用例数；忽略项不算通过。日志前缀均为 `C:/CodexLocal/wx-cli-final-fixture-`。

| 夹具 | 通过 / 忽略 | 日志后缀 |
| --- | --- | --- |
| attachment-refs | 87 / 0 | `attachment-refs.log` |
| native-image | 95 / 0 | `native-image.log` |
| mcp-image-listing-parity | 17 / 0 | `mcp-image-listing-parity.log` |
| mcp-protocol | 51 / 0 | `mcp-protocol.log` |
| mcp-readonly-security | 15 / 0 | `mcp-readonly-security-2.log` |
| mcp-image-security | 20 / 2 | `mcp-image-security-4.log` |
| native-attachment-security | 46 / 0 | `native-attachment-security-4.log` |
| mcp-audio | 170 / 0 | `mcp-audio-4.log` |
| mcp-voice-host | 1 / 0 | `mcp-voice-host-4.log` |
| mcp-voice-host-security | 19 / 1 | `mcp-voice-host-security-4.log` |
| wav-publish | 1 / 0 | `wav-publish-4.log` |
| asr-cache-security | 337 / 4 | `asr-cache-security-4.log` |
| emoticons-catalog | 82 / 0 | `emoticons-catalog-4.log` |
| emoticons-download | 135 / 2 | `emoticons-download-4.log` |
| sns-download | 97 / 1 | `sns-download-4.log` |
| mcp-cli | 32 / 0 | `mcp-cli-4.log` |

只读夹具先发现库存模块未注册，图片安全夹具先发现解析模块依赖及 XML 私有模块跨 crate 接线遗漏；均补入真实生产模块后重测，没有 mock 成功响应。早期失败日志保留。图片夹具新增直接 `roxmltree = "0.20"` 依赖，与主仓库一致；另一个附件夹具锁文件仅补齐已有路径依赖所需包，未升级已有包版本。

最终整仓逐项验收仍需将这些结果与完整目标逐项对照；阶段记录不代替该验收。
