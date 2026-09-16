# ASR 后端身份

共享解析位于 `src/service/operation_requests/asr_backend.rs`。内部身份只有 `whisper_cpp`、`python_whisper`、
`openai_compatible`，单次转录结果与批量报告显示规范名称。

| 名称 | 原生单次/显式批量/普通同步 MCP | 固定账号配置式批量 |
| --- | --- | --- |
| `whisper_cpp` | 要求显式程序和模型路径 | 从固定配置读取程序和模型路径 |
| `python_whisper` | 拒绝：该入口没有 Python 配置来源 | 读取 `local_whisper_model` |
| `openai_compatible` | 要求显式云端参数及上传许可 | 从固定配置读取服务参数，仍要求上传许可 |

原生默认 `--backend whisper_cpp`。配置式批量必须在固定账号配置中明确提供
`transcription_backend`；共享参数结构中的 `backend` 字段不覆盖固定配置，只有
`--explicit-backend` 才切换到显式原生参数。`local`、`openai`、
`explicit-open-ai`、缺失字段、未知名称和混合本地/云端参数均直接失败，不尝试
其他引擎或云端。

MCP 的 Python 路径仍要求宿主启用 `--configured-local-python`，且固定配置
必须明确写 `python_whisper`。仅提供规范后端名不构成宿主授权。
该入口仍禁止调用方指定临时目录或混入 cpp 路径、云参数；路径保护与账号绑定
由宿主执行。云端在读取凭据或音频前必须获得上传许可。

共享层只做纯解析和参数校验，不读取环境、凭据或文件，不扩展路径权限。
ASR API 凭据继续使用原入口的显式凭据文件或既有配置规则，不参与微信密钥迁移。
缓存身份同样只接受三个规范名称。含旧后端标签的缓存明确拒绝，需重新生成；
系统不会读取后静默改写或将旧标签映射到新引擎。

数据库关联快照从 `key_store::Store` 读取数据库密钥，不再读取明文 `keys_file`。
快照测试夹具须为合成 RuntimeContext 播种加密数据库密钥，不能只写 `all_keys.json`。
