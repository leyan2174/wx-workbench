# WeChat Database Decryptor 参考副本

本目录基于 [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt)，由本仓库修改维护，保留个人微信及共享功能，不包含企业微信专用入口；它不是未经修改的上游快照。第三方资产和许可证须按原声明保留。

当前产品入口是 Rust wx.exe，安装与使用见[仓库 README](../../README.md)。本目录用于源码参考、差异测试和明确选择的 Python 工作流；不能将其行为、授权或服务生命周期套用于 Rust。

## 文件导航

| 范围 | 入口 |
| --- | --- |
| 命令与配置 | main.py、config.py、setup.py |
| 数据库密钥与解密 | find_all_keys.py、find_all_keys_windows.py、decrypt_db.py |
| 图片 | decode_image.py、find_image_key.py、batch_decrypt_images.py |
| 聊天导出 | export_chat.py、export_all_chats.py、export_messages.py、chat_export_helpers.py |
| SNS | export_sns.py、export_sns_album.py、decrypt_sns.py |
| 表情 | emoticons.py、export_emoticons.py |
| 音频与转录 | voice_to_mp3.py、transcribe_chat.py |
| 查询与界面 | mcp_server.py、monitor.py、monitor_web.py、app_gui.py |
| 打包 | wechat_decrypt_launcher.py、WeChatDecrypt.spec、build.bat |

## 独立 Python 环境

以下命令在本目录执行，安装依赖会访问包源：

```powershell
py -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe main.py --help
```

先确认要使用的账号、配置与输出目录，再运行业务脚本。config.py 可以自动发现目录并写配置；不要在仅检查源码或运行 oracle 时导入它来寻找账号。测试以白名单 AST 与合成数据限制这些副作用。

进程内存读取可能需要额外权限，只能在明确授权后执行。日常只读查询不应为了方便默认提升整个环境权限。

## 输出与联网

数据库密钥、解密 SQLite、配置、转录缓存及导出内容均属私有数据，放在仓库外，不提交、不附入问题报告。Windows 默认 ACL 不等于已满足账号私有目录要求。

表情和 SNS 下载访问外部媒体服务；Python 命名模型可能下载权重。转录可选 local、whisper_cpp 或 openai，后端缺省与回退行为由该 Python 实现决定，不具备 Rust 显式云授权的相同契约。运行前核对配置与是否允许上传，不从缓存命中推断授权。

monitor_web.py 当前监听 0.0.0.0，可被其他网络接口访问。不要把它直接暴露到公网或不可信网络，也不能认为它具有 Rust 本地 Web 的 Host、Origin、令牌和 CSRF 防护。需要本地受控界面时使用根 README 指向的 Rust 入口。

## 导出约定

批量导出用 _export_index.json 按稳定 username 追踪文件，不以显示名作为唯一身份。CSV 计划的 blacklist 仅排除 export=0，whitelist 仅选择 export=1；展示统计列不代替实际日期筛选。

delta-only 是显式时间窗口输出，不覆盖完整聊天 JSON；无消息会话不生成空 delta。Python 格式见[聊天导出格式](docs/chat_export_format.md)，合成查询示例见[使用案例](USAGE.md)，Python 打包说明见[EXE_USAGE.md](EXE_USAGE.md)。

## 技术与许可

数据库解密使用相应 SQLCipher 页参数及 HMAC 校验，不能只凭内存候选的形状认定密钥有效。WAL 更新要检查周期和帧信息，不能把固定文件大小当作没有更新。

图片实现处理 XOR、V1/V2 AES 与相关媒体格式。SNS 视频密钥流使用供应商 WASM，Python 包装器需要 Node，见[资产说明](sns_media_wasm/README.md)；Rust 使用独立的嵌入式运行时，不能删除其编译输入。

保留本目录及依赖的许可证、版权头和来源说明，另见[第三方声明](../../THIRD_PARTY_NOTICES.md)。仅处理自己拥有或明确获准访问的数据。
