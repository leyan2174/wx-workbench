# 合成查询示例

以下名称、日期和消息主题全部虚构，仅展示查询参数，不包含真实聊天截图或结果。当前 Rust MCP 的参数与权限以[协议说明](../../src/mcp/PROTOCOL.md)为准；Python 参考实现的格式和错误行为可能不同。

## 最近会话与联系人

```json
{"name":"get_recent_sessions","arguments":{"limit":10}}
```

```json
{"name":"get_contacts","arguments":{"query":"示例","limit":20}}
```

结果可能包含联系人和消息原文，不应直接附到公开问题报告。

## 时间范围与分页

```json
{"name":"get_chat_history","arguments":{"chat_name":"synthetic-team@chatroom","start_time":"2025-01-01","end_time":"2025-01-07","limit":20,"offset":0}}
```

下一页将 offset 改为20。显示名可能重复，能确定 username 时优先使用精确身份；不要将同名会话合并。

## 搜索

```json
{"name":"search_messages","arguments":{"keyword":"示例项目","chat_name":["synthetic-user-a","synthetic-team@chatroom"],"limit":20,"offset":0}}
```

明确指定需要的范围，避免为了单一问题读取全部联系人或聊天。分页统计只代表已取得的数据，不能将一页结果当成完整活跃度排名。

## 新消息

```json
{"name":"get_new_messages","arguments":{}}
```

Rust MCP 此工具返回会话更新摘要，有会话级游标，不是保证无遗漏的完整消息流。换账号需建立新会话。

图片、语音解码和转录另有宿主输出、后端与上传授权要求，不由查询参数提供。配置与入口见[README](README.md)。
