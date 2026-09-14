# Python 打包程序说明

本页只描述 WeChatDecrypt.exe 的 Python/PyInstaller 路径，不是 Rust wx.exe 的安装说明。当前 Rust 入口见[仓库 README](../../README.md)。

## 入口

wechat_decrypt_launcher.py 无参数或使用 web 时启动 Python Web UI，其他参数分发到对应脚本：

```powershell
.\WeChatDecrypt.exe --help
.\WeChatDecrypt.exe status
.\WeChatDecrypt.exe export-all --write-plan-csv export_plan.csv
```

decrypt 会提取密钥并解密，export 会解密并导出，all 会串联多个步骤。运行前确认账号、权限及输出范围；不要把这些命令作为无副作用环境检查。

## 界面与输出

Web 工具箱包含个人微信和工具入口。聊天导出先选择会话再确认；此 Python 批量路径输出 JSON，不等于 Rust 的所有导出格式。语音 MP3 需要 PATH 中有 FFmpeg。任务停止不保证回滚已经写出的文件。

配置以实际加载结果为准，打包启动器使用 exe 所在目录作为运行基准。config.json、all_keys.json、decrypted、导出目录和转录缓存可能含密钥或聊天原文，应置于私有目录并限制访问，不与安装程序一起分发。

delta-only 要求明确时间起点，输出 deltas 下的清单及非空聊天 delta，不读取或覆盖完整聊天 JSON。

## 网络边界

Python monitor_web 当前监听 0.0.0.0，而不只限回环接口。不要直接用于公网或不可信局域网；其安全策略不能视为与 Rust 本地 Web 相同。云转录、媒体下载和模型下载也须在运行前分别核对。

## 构建

build.bat 使用 WeChatDecrypt.spec 打包。默认 Analysis 入口为 wechat_decrypt_launcher.py；app_gui.py 是独立 tkinter 界面。依赖安装和打包会生成环境与产物，按需运行，不属于普通文档检查。

更完整的文件导航、环境和许可说明见[参考副本 README](README.md)。这里只说明可用入口，不宣称打包结果已经完成安装部署验收。
