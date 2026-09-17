# 名称与运行约定

wx-workbench 是面向 Windows 微信本地数据的工作台，提供 CLI、MCP、本地 Web、账号服务、导出归档和媒体处理。

| 项目 | 名称 |
| --- | --- |
| 产品、Cargo 包、仓库内技能 | `wx-workbench` |
| GitHub 仓库 | `leyan2174/wx-workbench` |
| npm 主包 | `@leyan2174/wx-workbench` |
| npm Windows 包 | `@leyan2174/wx-workbench-win32-x64` |
| 命令与平台包入口 | `wx` / `bin/wx.exe` |
| Windows 发布附件 | `wx-workbench-windows-x86_64.exe` |
| MCP 名称 | `wx-workbench-mcp`，版本跟随 Cargo |

安装目录为 `%LOCALAPPDATA%\wx-workbench`。npm 的显式二进制覆盖变量为 `WX_WORKBENCH_BINARY`；未设置时使用 Windows 平台包。发布附件必须匹配上表名称。

`.wx-cli` 默认目录、`WX_CLI_*` 运行变量、命名管道、身份哈希域分隔符及磁盘标记是当前账号隔离与持久化约定。不要仅为统一品牌替换这些值或搬动用户数据。PATH 命中其他安装时，显式调用所需目录中的 `wx.exe`，再自行核对 PATH。

第三方项目名、版权及来源保持原署名，见[第三方说明](../THIRD_PARTY_NOTICES.md)。

安装器选择的合成回归入口为 `npm --prefix npm/wx-workbench test`；它不代表真实安装、联网下载或 npm 发布已验证。
