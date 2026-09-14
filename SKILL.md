---
name: wx-cli
description: "使用 wx-cli 查询和整理已授权的本地微信数据，包括会话、联系人、聊天历史、搜索、附件、导出和 MCP。"
---

# wx-cli

## 前置条件

仅处理用户拥有或明确授权的数据。确认 Windows x64、当前可执行文件及目标账号的配置、数据库和密钥。安装见 [README](README.md)。

不同账号使用不同的 `WX_CLI_CONFIG` 与 `WX_CLI_HOME`。普通查询优先使用已保存的密钥，不自动扫描进程、重启微信或捕获账号。

当前源库首页校验失败时，不以旧缓存继续查询，也不自动重新捕获密钥。首页校验通过不等于整库完好；解密和 WAL 处理仍须完成各自认证。参见[数据库认证边界](docs/architecture.md#数据库认证与缓存发布)。

```powershell
$account = Join-Path $env:USERPROFILE 'wx-cli-data/synthetic-account'
$env:WX_CLI_CONFIG = Join-Path $account 'config.json'
$env:WX_CLI_HOME = Join-Path $account 'runtime'
wx sessions -n 20 --json
wx contacts -n 20 --json
```

示例目录不代表用户真实账号。需要扫码、手机确认、购买服务、补充凭据、模型下载或新授权时，记录条件未满足并继续其他已授权工作。

## 查询

先列会话，选择真实联系人或群聊。聚合入口不能按普通聊天查询；显示名歧义时使用返回的精确标识，不猜测首个候选。

```powershell
$chat = '合成测试群'
wx history $chat -n 50 --json
wx search '测试关键词' --in $chat -n 20 --json
wx members $chat --json
wx stats $chat --json
```

限制数量与时间。区分空结果、超时、账号错误和资源缺失；只读重试也要保留前一次失败。不要在未检查副作用时重试发布、覆盖或回写。

监控使用 `wx toolkit monitor`，参数以 `--help` 为准。增量状态须完整保留，不截断会话或拆成多次查询；超出 8 MiB 或 100000 个会话时记录限额错误。查询传输层只连接现有 daemon，CLI 外层仍使用前台操作的生命周期。认证分块的未测量计时项为 `null`，不当作 0 ms。详见[监控入口](docs/daemon-entrypoints.md#监控与增量状态)。

## 导出与媒体

使用当前子命令的 `--help` 核对参数，不能把保留的上游脚本说明当成现行接口。输出与源库、配置、密钥和运行缓存分离。清理先预览；覆盖、回写、下载分别获得授权。

解码与识别是不同步骤。本地转录需要明确的程序、模型与依赖；命名 Python 模型可能下载。云端转录必须明确授权上传并指定端点、模型和凭据。缺少条件时不切换后端。参见[本地 ASR](src/toolkit/asr/LOCAL.md)与[云端授权](src/toolkit/asr/OPENAI.md)。

## 账号捕获

provider 和 DPAPI 规则见[账号密钥](docs/account-key-provider.md)。强制扫描与显式重启捕获只在授权后执行。重启捕获可能关闭微信并等待手机登录；没有授权时只说明条件，不试探性执行。

## MCP 与 Web

`wx mcp` 提供逐行 JSON-RPC，必须显式指定配置。工具参数不能选择账号、输出根、模型或凭据。注册工具不等于所有媒体条件均已满足。详见[协议](src/mcp/PROTOCOL.md)。

`wx toolkit run web` 启动本地界面，不暴露到不可信网络。只停止本任务创建且身份可验证的 daemon；不按进程名清理其他账号或用户应用。

## 结果与验证

原始返回可能包含联系人、正文、地址和文件路径。私人文件留在仓库外，终端与公开报告只给必要内容；测试报告使用状态与计数，不展示密钥或令牌。

以实际命令、产物和相关检查为依据，区分通过、失败与条件未满足。默认测试使用合成数据，隐藏子进程夹具只由父测试调用。参见[测试说明](tests/README.md)。
