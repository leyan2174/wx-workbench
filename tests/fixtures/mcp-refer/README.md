# 原生引用回复解码

实现位于 `src/daemon/query/mcp_refer.rs`。本工作流不修改 query、IPC、server 或 MCP 注册。

> 2026-09-07 文档核对：上述句子描述初次交付的写入范围。当前 `query.rs` 已注册并导出 `q_decode_refer`，不需要再次补接线。主仓定向入口为 `cargo test --bin wx daemon::query::mcp_refer -- --nocapture`。下方 oracle/临时副本说明保留，本次未执行，也不重建 golden 或覆盖旧日志。

## API

```rust
pub async fn q_decode_refer(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
) -> anyhow::Result<serde_json::Value>
```

调用方必须传入同一账号的 `DbCache` 和 `Names`，本模块不读取全局配置或传输层。
私有 resolver 优先精确 username（保留显式 wxid/群账号入口），随后按忽略大小写的
完整昵称、包含匹配两级查找。每级必须唯一，不再取首个或最短昵称；零候选返回未找到，
多候选返回 `exit_code=2`，均不访问数据库。旧共享 resolver 不变。

成功返回 `exit_code: 0`、`text`、`username`、`local_id`、实际 `create_time`、
逻辑分片 `source`，以及包含以下字段的 `refer`：

- `reply_text`, `refer_sender`, `refer_summary`
- `refer_type`, `refer_type_label`
- `refer_fromusr`, `refer_chatusr`, `refer_displayname`
- `refer_svrid`, `refer_createtime`（字符串，保留大整数精度）

业务失败返回 `exit_code: 1` 和 `text`，身份歧义返回 `exit_code: 2` 和 `text`，均无 `refer`。
缓存、缺失/未知分片、损坏 SQLite、SQL 类型或存储正文超限返回 `Err`；不从可读子集推断唯一性。

## 边界

- 已核验旧 `create_time=0` 表示不按时间筛选，不是只查时间戳零。
- 两个数值参数以有符号 SQLite i64 绑定，不缩窄到 u32。
- 跨片同 local_id 需时间消歧；相同时间及同片重复仍拒绝。先确认身份唯一，再检查消息类型。
- 读取前和读取完成后都复核未知分片；读取期间出现新分片时，不交付成功结果。
- 正数打包类型取低 32 位，负数不误认成 49。
- TEXT 即使标记 4 也保持文本；仅 BLOB 标记 4 走 zstd，损坏 TEXT UTF-8 拒绝。
- 只读 SQLite，不写导出文件，不返回原始引用 XML、密钥或 CDN 字段。
- 直接引用 `crate::message::xml` 和 `export_content::{refer_label, refer_summary}`，不重复编译或复制实现。
- XML 仍限 20,000 字符并拒绝 DTD/ENTITY；嵌套 type19 不能放宽引用正文上限。
- 存储正文最多 1 MiB，解压最多 128 KiB，之后仍检查 XML 字符上限。
- app type 沿用原生格式器的 ASCII 十进制子集，含符号和合法下划线，不接受全部 Python Unicode 数字语法。
- 引用时间保留字符串；可表示的 i64 时间按本地时区渲染，畸形或越界值仍保留在结构化字段中。

## 复现

```powershell
python tests/fixtures/mcp-refer/oracle.py
python tests/fixtures/mcp-refer/oracle.py --test-rust
```

AST oracle 只抽取旧纯函数及 `decode_refer`，使用临时合成 SQLite 与身份，
不导入 MCP 模块，不访问真实账号。`--write` 可重建 golden；现有 30 个对照用例保持不变。

Rust runner 把 src、tests、Cargo 文件和编译期 WASM 资源复制到临时目录，只在副本中
补充尚不存在的模块注册。它直接编译共享 XML/摘要接口以及真实 `DbCache`，复用构建缓存，
不修改工作树注册文件。测试通过受控调度在 SQLite 读取后、结果恢复前加入新分片，无 sleep 竞态。

完整日志：`C:/CodexLocal/日志/mcp-refer-check.log` 和 `mcp-refer-test.log`。
