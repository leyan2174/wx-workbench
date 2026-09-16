# wx-workbench 当前命名与发布

本项目由 wx-cli 演进而来，当前同时提供 CLI、MCP、本地 Web、账号服务、导出归档和媒体处理。产品改名为 **wx-workbench（微信本地数据工作台）**，以反映当前边界。它仍然是面向微信本地数据的工具，不宣称已成为支持任意社交平台的通用系统。

## 名称对应

| 项目 | 新名称或保留约定 |
| --- | --- |
| 产品、Cargo 包、仓库内技能 | `wx-workbench` |
| GitHub 仓库名称 | `leyan2174/wx-workbench` |
| npm 主包 | `@leyan2174/wx-workbench` |
| npm Windows 包 | `@leyan2174/wx-workbench-win32-x64` |
| 主要命令 | 保留 `wx` |
| 可执行入口 | 仅发布 `wx`；移除 `wx-toolbox` binary |
| 主发布附件 | 仅 `wx-workbench-windows-x86_64.exe` |
| MCP 展示名称 | `wx-workbench-mcp`；版本跟随 Cargo 包版本 |

## 为什么仍会看到 wx-cli

以下属于第三方来源或当前运行时/数据边界，不属于工具发布兼容入口；本次发布清理不迁移、不重写用户数据：

- `.wx-cli` 默认配置/运行目录及运行时 `WX_CLI_*` 环境变量不在本次发布清理范围；npm 专用的旧 `WX_CLI_BINARY` 已不再读取。
- 真实用户磁盘上的旧安装、配置和历史文件保留，不自动搬迁或删除。
- 查询与任务管道名、运行身份哈希的域分隔符，以及磁盘格式标记。这些值参与账号隔离或持久化；直接替换会使已有账号身份、任务历史或缓存失配。
- 原版 `jackwener/wx-cli` 的仓库地址、版权与致谢、历史审查记录、原有日志文件路径。不能将第三方来源重新署名为本项目。
- 已删除的 `vendor/wechat-decrypt` 参考实现仍通过 README、`THIRD_PARTY_NOTICES.md`、版权记录和 Git 历史保留来源说明；当前源码树不再携带或运行该目录。

npm 二进制覆盖仅支持 `WX_WORKBENCH_BINARY`，否则只查找新的 Windows 平台包。
安装器固定使用 `%LOCALAPPDATA%\wx-workbench`，不再探测或复用旧安装目录。
Release 缺少 `wx-workbench-windows-x86_64.exe` 时直接失败，不回退旧附件名。
发布工作流不再构建、复制或上传旧 binary/附件；npm 平台包中的正式入口仍为 `bin/wx.exe`。
若用户 PATH 仍优先命中旧安装，可显式调用新目录内的 `wx.exe` 或自行调整 PATH；
本次清理不会自动删除旧 PATH 项或磁盘文件。

发布选择回归可运行 `npm --prefix npm/wx-workbench test`（测试环境为 Node 20）。这些测试使用合成环境，
不联网下载、不发布 npm 包、不执行真实安装、不修改用户 PATH。

## 外部发布步骤

2026-09-16 已通过 GitHub API 将仓库更名为 `leyan2174/wx-workbench`，核验新地址并更新本 checkout 的 origin。其他 checkout 需自行更新远端地址。新 npm scope 的发布权限和 `NPM_TOKEN` 仍需在发布前核对；本次没有发布包、创建版本或推送代码修订。

使用新地址克隆：

```powershell
git clone https://github.com/leyan2174/wx-workbench.git
cd wx-workbench
```

GitHub 改名不会替代代码与文档的许可证、来源说明或发布风险审查；参见 [DMCA 与文档发布风险](dmca-and-publication-risk.md)。
