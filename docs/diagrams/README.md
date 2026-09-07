# 架构图导航

本目录图件直接来自 [架构正文](../architecture.md) 的 28 个 Mermaid 块：27 张现行结构或流程图，以及 1 张原始目标架构的历史设计图（25）。不再维护独立的手工节点副本。生成器保存每块最新标题、当前/历史分类、来源行号、SHA-256 与尺寸，见 [渲染报告](render-report.json)。正文改动后需要重新生成；图的颜色只用于布局，不表示测试通过。

## 常用入口

| 内容 | SVG | PNG | 正文图号 |
| --- | --- | --- | --- |
| 当前总览 | [SVG](runtime-current.svg) | [PNG](runtime-current.png) | 02 |
| 旧总览链接的当前别名 | [SVG](current-runtime.svg) | [PNG](current-runtime.png) | 02 |
| MCP 语音完整时序 | [SVG](mcp-voice-flow.svg) | [PNG](mcp-voice-flow.png) | 23 |
| ASR 与视频 WASM | [SVG](asr-wasm-wiring.svg) | [PNG](asr-wasm-wiring.png) | 09 + 10 |
| SNS 时间线与相册 | [SVG](sns-cache-publish.svg) | [PNG](sns-cache-publish.png) | 11 + 12 |

总览只承担导航，详细关系在下表中，不以删减业务链路来压缩一张长图。两张组合图按原 Mermaid 块上下排列；其源文件分别是 09/10 和 11/12 的独立 .mmd，不是可一次解析的多图 .mmd。ASR 三后端与共享监督详图见 18；图 10 包含保留的企微实现，不纳入本轮个人微信验收。

## 完整图集

| 图号 | 主题 | 文件 |
| --- | --- | --- |
| 01 | 共享私有文件权限 | [Mermaid](diagram-01.mmd) / [SVG](diagram-01.svg) / [PNG](diagram-01.png) |
| 02 | 运行总览 | [Mermaid](diagram-02.mmd) / [SVG](diagram-02.svg) / [PNG](diagram-02.png) |
| 03 | 入口与配置 | [Mermaid](diagram-03.mmd) / [SVG](diagram-03.svg) / [PNG](diagram-03.png) |
| 04 | MCP 与有界传输 | [Mermaid](diagram-04.mmd) / [SVG](diagram-04.svg) / [PNG](diagram-04.png) |
| 05 | 历史、联系人与引用 | [Mermaid](diagram-05.mmd) / [SVG](diagram-05.svg) / [PNG](diagram-05.png) |
| 06 | 图片解码与发布 | [Mermaid](diagram-06.mmd) / [SVG](diagram-06.svg) / [PNG](diagram-06.png) |
| 07 | MCP 语音准备 | [Mermaid](diagram-07.mmd) / [SVG](diagram-07.svg) / [PNG](diagram-07.png) |
| 08 | 缓存、数据库与音频 | [Mermaid](diagram-08.mmd) / [SVG](diagram-08.svg) / [PNG](diagram-08.png) |
| 09 | ASR 批量与缓存 | [Mermaid](diagram-09.mmd) / [SVG](diagram-09.svg) / [PNG](diagram-09.png) |
| 10 | 视频 WASM 与保留企微实现 | [Mermaid](diagram-10.mmd) / [SVG](diagram-10.svg) / [PNG](diagram-10.png) |
| 11 | SNS 时间线 | [Mermaid](diagram-11.mmd) / [SVG](diagram-11.svg) / [PNG](diagram-11.png) |
| 12 | SNS 相册 | [Mermaid](diagram-12.mmd) / [SVG](diagram-12.svg) / [PNG](diagram-12.png) |
| 13 | 聊天批量导出 | [Mermaid](diagram-13.mmd) / [SVG](diagram-13.svg) / [PNG](diagram-13.png) |
| 14 | Delta 与计划 | [Mermaid](diagram-14.mmd) / [SVG](diagram-14.svg) / [PNG](diagram-14.png) |
| 15 | 导出正文与验证 | [Mermaid](diagram-15.mmd) / [SVG](diagram-15.svg) / [PNG](diagram-15.png) |
| 16 | 文件、Web 与监控 | [Mermaid](diagram-16.mmd) / [SVG](diagram-16.svg) / [PNG](diagram-16.png) |
| 17 | 双入口与配置向导 | [Mermaid](diagram-17.mmd) / [SVG](diagram-17.svg) / [PNG](diagram-17.png) |
| 18 | ASR 批量详细流程 | [Mermaid](diagram-18.mmd) / [SVG](diagram-18.svg) / [PNG](diagram-18.png) |
| 19 | Web 工作流 | [Mermaid](diagram-19.mmd) / [SVG](diagram-19.svg) / [PNG](diagram-19.png) |
| 20 | 清理计划与执行 | [Mermaid](diagram-20.mmd) / [SVG](diagram-20.svg) / [PNG](diagram-20.png) |
| 21 | 数据库解密时序 | [Mermaid](diagram-21.mmd) / [SVG](diagram-21.svg) / [PNG](diagram-21.png) |
| 22 | MCP 图片时序 | [Mermaid](diagram-22.mmd) / [SVG](diagram-22.svg) / [PNG](diagram-22.png) |
| 23 | MCP 语音时序 | [Mermaid](diagram-23.mmd) / [SVG](diagram-23.svg) / [PNG](diagram-23.png) |
| 24 | DAT 解码 | [Mermaid](diagram-24.mmd) / [SVG](diagram-24.svg) / [PNG](diagram-24.png) |
| 25 | 原始目标架构（历史设计，非当前文件映射） | [Mermaid](diagram-25.mmd) / [SVG](diagram-25.svg) / [PNG](diagram-25.png) |
| 26 | 账号运行身份 | [Mermaid](diagram-26.mmd) / [SVG](diagram-26.svg) / [PNG](diagram-26.png) |
| 27 | 转账解码 | [Mermaid](diagram-27.mmd) / [SVG](diagram-27.svg) / [PNG](diagram-27.png) |
| 28 | 位置解码 | [Mermaid](diagram-28.mmd) / [SVG](diagram-28.svg) / [PNG](diagram-28.png) |

图 25 仅保留原始设计意图；其中节点不代表当前文件映射，过渡箭头也不是当前待迁移状态。其余图件中的保留企微旁支仍不属于本轮验收目标。

## 当前验证与边界

daemon 任务归并后的现行 28 张图与 5 个别名已重新生成，日志 `C:/CodexLocal/wx-cli-daemon-service-diagrams.log`，报告以当前 [render-report.json](render-report.json) 为准。图 02/03/04/16/19 已同步任务所有权和 Web 关闭边界；本次实际回看总览及图 19，详见 [视觉复核](visual-review.md)。生产证据见[迁移记录顶部](../rust-migration.md)，下述旧计数仅表示此前文档同步时点。

### 此前验证（历史）

2026-09-07 本地 Mermaid / Playwright / Edge 重新生成全部 28 张 SVG、PNG 与 .mmd，以及上述五个兼容图名。流程图节点重叠和标签越界检查为 0；时序图完成渲染，不宣称自动验证全部连线或文字碰撞。总览、ASR/WASM、SNS 与 MCP 语音 PNG 已实际回看，详见 [视觉复核](visual-review.md)。完整执行输出见 [生成日志](current-sync-render.log)。

生产验证统一引用 [迁移与回归清单](../rust-migration.md)：精简后 Rust 20 套件 1325 passed / 0 failed / 11 ignored，个人前端 61/0；8 次默认忽略项的可选执行另行通过，check 仍有 9 条警告。本次图生成没有重新运行 Cargo，也不证明真实账号、模型/GPU、云服务或安装部署已验收。

直接 exe 不需要 Node；文档生成使用 Node 与浏览器。Python Whisper/PyTorch 推理兼容路径仍保留：配置型批量转录缺省 local 时可选择它，MCP 则需显式宿主开关。命名模型可能下载权重，不应写成保证全离线。企业微信排除本轮目标，既有代码和图中旁支保留。

## 离线复现

在仓库根目录，传入已经安装的 Playwright 模块与本地 Mermaid 浏览器包：

```powershell
node docs/diagrams/export_png.cjs <playwright-module-path> <local-mermaid-bundle.js>
```

本机已验证命令：

```powershell
node docs/diagrams/export_png.cjs C:/Users/leyan/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright C:/Users/leyan/.vscode/extensions/shd101wyy.markdown-preview-enhanced-0.8.34/crossnote/dependencies/mermaid/mermaid.min.js
```

不下载依赖、不联网、不读取私有账号。需要已安装 Edge；其他机器应替换本地依赖路径。生成器拒绝正文图数变化或别名主题不符的情况，防止默默映射错误。图数/顺序变更时先审阅别名表。正文在生成期间变化也会失败，需重新生成一致快照。

旧 `render_architecture.py` 已停止作为当前图生成入口；运行会明确报错并指向以上命令，避免旧列表覆盖最新图。

## 历史证据

更新前的五类图、两份说明、生成器及报告保存在 [历史目录](history/pre-personal-sync/README.md)。图件与报告保留原始内容，归档说明仅补充历史标识及相对导航修正；其中“当前”“未迁移”“待跑”等文字只指当时状态。

根目录的 `source-evidence.json`、`current-runtime.json`、`current-runtime.layout.json` 和除 `current-sync-render.log` 外的旧 audit/render/source-check 日志仍是历史材料，不是当前图输入或新回归证据。它们不因兼容 SVG/PNG 图名更新而自动获得新的时点；当前生成只认正文与 `render-report.json`。
