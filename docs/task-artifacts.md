# 任务产物交付

此接口交付 `export_all` 的聊天目录产物和 `export_history` 的单会话历史文档，复用 daemon 任务、worker 和受控发布记录。它不代表所有 CLI 导出形式均已接入任务，也不提供任意文件浏览、路径读取或命令执行。

## 单会话历史

`export_history` 使用现有历史查询与渲染，支持 Markdown、TXT、JSON、YAML。一次任务选择一个会话及一种格式，输出文件由宿主确定。原 `wx export` 仍是前台操作，stdout 和显式输出路径行为不改成后台任务。

```text
wx tasks submit export_history --chat wxid_example --since 2026-09-01 --until 2026-09-18 --limit 10001 --format yaml --wait
wx mcp --tasks --task-kind export_history --task-allow-artifact-read
```

对应 `submit_task` 的参数：

```json
{"idempotency_key":"<64位小写十六进制ID>","kind":"export_history","options":{"history_export":{"chat":"wxid_example","since":"2026-09-01","until":"2026-09-18","limit":10001,"format":"yaml"}}}
```

`chat` 优先按账号内精确 username 解析，唯一名称也可使用；歧义或不存在会失败。`limit` 默认 500，必须为正整数，不额外设置一万条业务上限。MCP/Web 使用 JSON 安全整数。任务查询超时为 300 秒，响应沿用 32 MiB 查询上限，渲染文件上限为 64 MiB；超限明确失败，不承诺无限大小或流式查询全部历史。

日期按宿主本地时间解析。`since` 包含开始边界；纯日期 `until` 包含当天 `23:59:59`，显式时刻保持原值。此规则与批量计划的 `end` 不同，不可互换。

单会话结果的 `scope` 为 `chat_history`，包含格式、查询摘要、发布状态、产物数量及诊断；查询摘要未知时为 `null`，不是零条成功。它不包含目录导出的会话统计字段。任务成功与取得文件仍是不同步骤，文件通过下述产物 ID 接口读取。

宿主允许 `export_history` 即授权该任务在固定目录写入文本，不需要媒体写入授权；读取文件仍独立要求 `--task-allow-artifact-read`。模型不能传输出路径、启用授权或借此下载媒体。单文件发布和产物登记不是同一原子事务，强杀发生在两者之间时不保证文件可通过任务接口读取，不扫描并收编未登记文件。

## 状态与结果

`Task.status` 和退出码表示整个任务的执行结果。`Task.result.scope = chat_directory` 只描述聊天目录步骤；后续朋友圈步骤失败时，聊天结果可以为成功而整个任务为失败。

- `finalized`：聊天步骤是否返回终结报告；取消或中断后恢复的已发布清单不等于完整终结报告。
- `outcome`：聊天步骤的业务结果，不替代任务状态。
- `artifact_count`：已登记产物数，不等于导出记录数。
- `artifacts_complete`：已确认发布的聊天清单是否登记完整，不保证全部选择集成功。
- `diagnostics`：稳定类别与计数，不包含原始错误链、密钥或物理路径。

失败、取消或中断不撤回此前已经完整发布的聊天文件。只有通过账号绑定和发布清单验证的产物可交付；暂存文件及无清单的散落文件不会因存在于磁盘就成为可下载产物。服务重启不承诺自动断点续跑。

受监督 worker 在每个聊天发布后登记产物，再进入下一聊天。收尾恢复响应取消、停机和超时，保留已验证的登记前缀并明确标记不完整。强杀可能发生在发布与登记之间，此窗口中的文件不保证立即可读；若取消时还没有登记完成的聊天，清单可以为空。文件保留、登记完成、读取可用与整项业务成功是不同状态。

取消检查覆盖文件与 1 MiB 读取块，失败读取也消耗预算；仍需完成最多 4 MiB 的检查点、32 MiB 的索引装载及原子提交，并等待当前系统读取返回，因此不承诺硬实时中断。收尾超时使用 `artifact_finalization_timeout` 诊断，中断使用 `export_interrupted`，不能当作完整登记成功。

## 参数

`export_all` 在既有选择、格式和媒体选项之外接受：

| 参数 | 默认值 | 约束 |
| --- | --- | --- |
| `dry_run` | `false` | 只产生计划结果；不能与 `include_sns` 同时启用。 |
| `max_media_bytes` | 64 MiB | 显式值为 1 字节至 500 MiB。 |
| `max_total_media_bytes` | 每聊天 2 GiB | 不小于生效的单件预算，不超过 16 GiB。 |

关闭媒体时拒绝显式媒体预算，其他任务类型拒绝这些导出专用选项。dry-run 不授予扫描、下载或媒体写入权限。普通导出的媒体缺失诊断及 `allow_missing_media` 语义不变。

## MCP

任务管理授权与文件内容读取授权分开。宿主显式启用 `--tasks --task-allow-artifact-read` 后，才开放 `list_task_artifacts` 和 `read_task_artifact`。模型参数不能启用这一权限；读取已有产物不要求授予提交新导出的权限。

允许提交聊天导出并读取产物的宿主示例：

```powershell
wx mcp --tasks --task-kind export_all --task-allow-media-write --task-allow-artifact-read
```

任务完成后，使用 daemon 返回的任务 ID 获取清单：

```json
{"name":"list_task_artifacts","arguments":{"id":"<task-id>","offset":0,"limit":50}}
```

使用清单中返回的不透明 `artifact_id` 读取字节：

```json
{"name":"read_task_artifact","arguments":{"id":"<task-id>","artifact_id":"<artifact-id>","offset":0,"max_bytes":65536}}
```

清单分页上限为 100。服务字节块上限为 1 MiB；MCP 还会按宿主帧预算收紧块大小，不能把服务上限直接当成 MCP 可用载荷大小。响应包含 `data_base64`、`bytes_read`、`next_offset`、`eof`、总大小和 SHA-256。逐块推进偏移，最后核对长度及完整 SHA-256；不能拼接来自不同摘要的块。

清单中的 `name` 只用于显示，可以重名，不能代替产物 ID，也不能作为读取路径。仅终态任务允许读取；MCP 断连不取消任务，重新连接后使用原任务 ID 和幂等键继续查询。

## CLI

```powershell
wx tasks artifacts <task-id> --offset 0 --limit 50
wx tasks read-artifact <task-id> <artifact-id> --offset 0 --max-bytes 65536
```

两个命令返回与任务 RPC 对应的 JSON 清单或 Base64 字节块，不接受任意源文件路径。原有任务详情字段保持兼容；新增产物接口以任务和产物 ID 定位，不通过详情中的输出目录读取文件。

## 下载与保留

Web 下载使用受控分块读取并以附件交付，禁止嗅探及缓存。HTML 等用户内容不在工作台同源内联执行；不支持 HTTP Range 时明确返回 416，而非伪造范围结果。

| HTTP 接口 | 行为 |
| --- | --- |
| `GET /api/tasks/{id}/artifacts?offset=0&limit=50` | 已认证的分页清单。 |
| `POST /api/tasks/{id}/artifacts/{artifact_id}/ticket` | 已认证且通过 CSRF 检查后签发单产物的 60 秒下载票据，返回相对 URL 和 `expires_at`。 |
| `GET /api/tasks/{id}/artifacts/{artifact_id}/download` | 使用正常认证头或有效下载票据流式取得附件。 |

票据绑定当前运行实例、账号、任务、产物及有效期，不能用于其他 API。浏览器通过票据 URL 发起原生附件下载；不把完整工作台令牌放入 Cookie 或下载 URL，不依赖跨端口共享的 localhost Cookie 授权。票据在到期前可能重复使用，不承诺一次性消费。

读取时再次验证文件身份与内容，验证和字节读取使用同一个受保护句柄。文件被替换、改写、移除或变成重解析路径时拒绝读取。清单元数据不是文件永久不变的承诺；下载中途失败时不能把已收到的前缀当作完整文件。

任务历史与磁盘产物的保留周期分开：淘汰任务记录不删除用户产物，任务接口也不承诺在历史记录淘汰后仍提供读取。清理文件需经过单独的受控清理流程，不由查询或下载隐式执行。

任务生命周期、账号固定与幂等规则见[后台任务](daemon-tasks.md)，其他尚未对等公开的导出能力见[能力矩阵](capability-matrix.md)。
