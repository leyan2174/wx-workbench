# 四图视觉复核

## 当前：SNS 原生时间线路由

2026-09-07，仅原位更新 `runtime-current.svg/png` 与 `sns-cache-publish.svg/png`，沿用现有 Python 列表图源、Fireworks 校验及 Playwright/Edge 渲染；没有新增平行图片。总图仍为 17 行，其他业务行保留；SNS 专图由 6 行扩为 7 行，区分生产账号 Update、静态 fresh/update、缓存平铺/嵌套及共享逐文件发布。相册的 Feed 与媒体回退顺序不变。

`visual_review: passed`：两张当前 PNG 已实际回读，未见文字或连线重叠。总图 3280 × 9840，SNS 专图 3280 × 4440；总图缩略显示很长，应使用原始 PNG 或可缩放 SVG 阅读。每图 xml、markers、collisions、geometry、composition 五项通过；浏览器六类文字溢出/碰撞检查均为零，详见 `render-report.json`。其他两图本轮未重绘，其报告条目仍是历史结果。

主任务确认全量日志 `C:/CodexLocal/wx-cli-sns-timeline-all-retest.log` exit 0，20 targets、1142 次通过、0 失败、11 忽略，含 host 8/core 6/CLI 7；计数包含重复执行。check 仍待主任务确认，ASR、exportall、GUI 和清理尚有剩余，不代表整体迁移完成。本轮未运行 cargo 或访问真实数据。

使用 `--defer-evidence`，不修改 `source-evidence.json`；图上明确标记旧快照与待封存状态，不声称本轮 source-check 通过。开始时该文件 SHA-256 为 `6DCE8361442CA20709776AAC3A054AAFF9A40D4CD6F1FD1C5D6D13E04F7F69AD`。完整标准输出/错误输出保存于 `C:/CodexLocal/wx-cli-sns-timeline-diagrams.log`，包括首次受限读取锚点失效；将锚点从 album 移到共享 publish 后成功重绘。

## 历史：SNS 原生相册阶段

2026-09-07，沿用 `render_architecture.py` 和 `export_png.cjs` 原位更新四张图，不新增平行图。最终通知前使用 `--defer-evidence`；收到权威结果后正常生成并封存源码证据。以下为该阶段历史生成物，不作为当前时间线结果。

| 图 | 最新像素尺寸 | 本轮复核 |
| --- | --- | --- |
| runtime-current.png | 3280 × 9840 | 扩展至 17 行，原业务链保留；新增固定账号 Feed、媒体顺序、绑定与逐文件发布；无可见重叠 |
| sns-cache-publish.png | 3280 × 3900 | 六行展示原生相册、video-only cache、懒加载、adopt 来源未核验与旧入口边界 |
| asr-wasm-wiring.png | 3280 × 4440 | 相册节点改为原生调用；移除页脚 Python/Node 相册网络旧说法 |
| mcp-voice-flow.png | 3280 × 4440 | 业务链保留；1001 计数标为历史，不作为 SNS 最终结果 |

`visual_review: passed`：四张实际 PNG 已逐张回读。每张 SVG 五项 Fireworks 检查通过；Edge 实测六类溢出/碰撞计数均为 0，见 `render-report.json`。运行总图缩略图较长，完整 SVG/PNG 保持原始字号和行距，不缩小字体塞入旧画布。

本轮未运行 cargo。最新权威结果为 19 targets、1121 次通过、0 失败、11 忽略；相册 CLI 12/0，包含新增加密视频与最终 cache mtime 测试。MSVC check exit 0、10 条 unused 警告；相关 rustfmt check 通过由主任务确认。两份 complete-final 日志已读取，日志哈希纳入 `source-evidence.json`。107/0/2、首次全量 1119/0/11 为历史，不再作为最终结果。1121 是执行次数，非唯一功能数，不代表全部旧功能迁移或真实账号验收。

完整命令输出（包括早期读取截断前的原文和一次 rg 通配路径错误）保存在 `sns-architecture-work.log`。`git diff --check` 无输出，但本目录含未跟踪文件，该命令不构成全部新文档内容校验。

## 历史：Status/Emoticons

2026-09-07，status/emoticons 最终文档图更新，使用既有 Fireworks 图源及 Playwright/Edge 导出后，使用 view_image 逐张回读全部实际改动 PNG。

| 图 | 像素尺寸 | 视觉结果 |
| --- | --- | --- |
| runtime-current.png | 3280 × 7680 | 十三行完整；原 image_metadata 链保留；新增 run status、表情 prepare/offline、隔离 DbCache、catalog、过滤/预览与守卫下载发布链；catalog 跨行线避开标题和节点 |
| sns-cache-publish.png | 3280 × 2280 | 三行节点完整；缓存回填箭头避开节点，页脚完整 |
| asr-wasm-wiring.png | 3280 × 4440 | 七行节点完整；既有业务链未改，统一快照标题更新，无截字或连线穿字 |
| mcp-voice-flow.png | 3280 × 4440 | 七行节点完整；标题及右下当前基线均为 1001/0/10、2 警告，额外 FFmpeg 单列，无重叠 |

`visual_review: passed`。四张 SVG 各通过 xml、markers、collisions、geometry、composition 检查；浏览器文字溢出、画布越界、文字互撞、文字/节点、连线/文字、连线/节点检查均为零，见 render-report.json。首次回读发现语音图当前节点残留 962/9，已修正并重新生成、回读运行总图与语音图；其余两图未再改变业务内容。

图像缩略显示不改变本地原始像素尺寸；SVG 可缩放查看。普通全测为 16 组 1001 次通过、0 失败、10 忽略，额外 FFmpeg 1 项通过不并入。该计数不是唯一用例数；图形复核不等于全部旧功能迁移、用户私有账号、真实模型质量或已安装版本验收。历史三图复核尺寸不作为当前生成物尺寸。
