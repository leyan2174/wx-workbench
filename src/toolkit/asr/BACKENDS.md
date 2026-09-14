# ASR 后端身份与兼容

共享解析位于 `backend.rs`。内部身份只有 `whisper_cpp`、`python_whisper`、
`openai_compatible`，单次转录结果与批量报告显示规范名称。

| 输入 | 原生单次/显式批量/普通同步 MCP | 配置式旧批量 |
| --- | --- | --- |
| `local` | whisper.cpp | Python Whisper |
| `whisper_cpp` | whisper.cpp，要求显式程序和模型路径 | whisper.cpp，保留旧配置路径与模型发现规则 |
| `python_whisper` | 明确拒绝：该入口没有 Python 配置来源 | Python Whisper，读取 `local_whisper_model` |
| `openai_compatible` | 显式云端参数及上传许可 | 旧 OpenAI 配置及显式上传许可 |
| `openai` / `explicit-open-ai` | `openai_compatible` 的兼容别名 | 同左 |

配置式批量缺少 `transcription_backend` 时仍选择 Python Whisper；原生默认
`--backend local` 仍选择 whisper.cpp。配置式批量可用规范 `--backend` 名称
显式选择后端；默认 `local` 参数仍让固定配置决定引擎。未知名称、非法配置、
混合本地/云端参数直接失败，不尝试其他引擎或云端。

MCP 的 Python 路径仍要求宿主启用 `--configured-local-python`，且固定配置
必须明确写 `local` 或 `python_whisper`。仅提供规范后端名不构成宿主授权。
该入口仍禁止调用方指定临时目录或混入 cpp 路径、云参数；路径保护与账号绑定
由宿主执行。云端在读取凭据或音频前必须获得上传许可。

共享层只做纯解析和参数校验，不读取环境、凭据或文件，不扩展路径权限。
ASR API 凭据继续使用原入口的显式凭据文件或既有配置规则，不参与微信密钥迁移。
底层旧缓存摘要/身份保留，以免名称规范化造成已有缓存失效；缓存中已有的历史
后端标签仍可见，不代表自动切换引擎。

数据库关联快照从 `key_store::Store` 读取数据库密钥，不再读取明文 `keys_file`。
快照测试夹具须为合成 RuntimeContext 播种加密数据库密钥，不能只写 `all_keys.json`。
