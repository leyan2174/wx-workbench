use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 内部语音响应含最多 16 MiB SILK 的 base64；不改变公开 MCP 帧上限。
pub const MAX_PREPARED_VOICE_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

/// CLI 向 daemon 发送的请求（换行符分隔 JSON，与 Python 版兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    /// 固定只读查询的缓存、解密、WAL 和查询阶段计时，不返回聊天内容。
    LatencyProbe {
        limit: usize,
    },
    /// 内部联系人身份解析；不查询消息或媒体，不暴露为新的 MCP 工具。
    ResolveChat {
        chat: String,
    },
    ExportChatList,
    /// 按实际消息表枚举目录，包含已不在会话列表中的历史聊天。
    ExportDirectoryCatalog,
    ExportChatByUsername {
        username: String,
    },
    /// 目录导出专用字段；不扩充默认紧凑聊天 JSON 的公开契约。
    ExportDirectoryByUsername {
        username: String,
    },
    /// 原始增量消息；按精确 username，保留正文原始字节供 UID 计算。
    ExportDelta {
        username: String,
        start: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        end: Option<i64>,
    },
    ExportChat {
        chat: String,
    },
    DecodeLocation {
        chat: String,
        local_id: i64,
        #[serde(default)]
        create_time: i64,
    },
    DecodeTransfer {
        chat: String,
        local_id: i64,
        #[serde(default)]
        create_time: i64,
    },
    DecodeRefer {
        chat: String,
        local_id: i64,
        #[serde(default)]
        create_time: i64,
    },
    /// 只读返回当前账号缓存中的外层文件引用，不下载或创建文件。
    DecodeFileMessage {
        chat: String,
        local_id: i64,
        #[serde(default)]
        create_time: i64,
    },
    /// item_index 与聊天记录展开内容一致，使用从零开始的索引。
    DecodeRecordItem {
        chat: String,
        local_id: i64,
        item_index: i64,
        #[serde(default)]
        create_time: i64,
    },
    /// 图片写出由 MCP 宿主显式配置；公开工具 schema 不接受以下路径字段。
    DecodeImage {
        chat: String,
        local_id: i64,
        #[serde(default)]
        create_time: i64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_key_file: Option<String>,
    },
    /// daemon 仅准备音频；local_id 是媒体记录 ID，WAV 由 MCP 宿主发布。
    DecodeVoice {
        chat: String,
        local_id: i64,
    },
    /// daemon 不读取识别后端或凭据；宿主校验准备数据后执行显式转录。
    TranscribeVoice {
        chat: String,
        local_id: i64,
    },
    Sessions {
        #[serde(default = "default_limit_20")]
        limit: usize,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    History {
        chat: String,
        #[serde(default = "default_limit_50")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_type: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        msg_types: Option<Vec<i64>>,
        #[serde(default, skip_serializing_if = "is_false")]
        oldest_first: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    Search {
        keyword: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        chats: Option<Vec<String>>,
        #[serde(default = "default_limit_20")]
        limit: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_type: Option<i64>,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    Contacts {
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        #[serde(default = "default_limit_50")]
        limit: usize,
        #[serde(default, skip_serializing_if = "is_false")]
        legacy_view: bool,
    },
    ContactTags,
    TagMembers {
        tag_name: String,
    },
    /// 只读媒体分片元数据；音频大小来自 VoiceInfo，不读取音频正文。
    VoiceMessages {
        chat: String,
        #[serde(default = "default_limit_20")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
    },
    Unread {
        #[serde(default = "default_limit_20")]
        limit: usize,
        /// 按会话类型过滤：private / group / official / folded / all，支持多选
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    Members {
        chat: String,
    },
    NewMessages {
        /// 上次检查时各会话的 last_timestamp 快照（username -> ts）
        /// None 表示首次运行，会返回 new_state 供下次使用
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<HashMap<String, i64>>,
        #[serde(default = "default_limit_200")]
        limit: usize,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    Stats {
        chat: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    Favorites {
        #[serde(default = "default_limit_50")]
        limit: usize,
        /// 类型过滤：1=文本,2=图片,5=文章,19=名片,20=视频
        #[serde(skip_serializing_if = "Option::is_none")]
        fav_type: Option<i64>,
        /// 内容关键词搜索
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
    },
    /// 朋友圈互动通知（点赞 + 评论）
    SnsNotifications {
        #[serde(default = "default_limit_50")]
        limit: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        /// 包含已读通知（默认仅未读）
        #[serde(default)]
        include_read: bool,
    },
    /// 朋友圈时间线（按时间 / 作者筛选帖子）
    SnsFeed {
        #[serde(default = "default_limit_20")]
        limit: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        /// 作者昵称 / 备注名 / 微信 username，模糊匹配
        #[serde(skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// 查询公众号文章推送（biz_message_*.db 分片）
    BizArticles {
        #[serde(default = "default_limit_50")]
        limit: usize,
        /// 公众号名称过滤（模糊匹配 display name，None = 全部）
        #[serde(skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        /// 只看有未读消息的公众号，每个公众号取最新 1 篇
        #[serde(default)]
        unread: bool,
    },
    /// 朋友圈全文搜索（匹配 contentDesc）
    SnsSearch {
        keyword: String,
        #[serde(default = "default_limit_20")]
        limit: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// 重新加载 daemon 的联系人缓存
    ReloadConfig,
    /// 列出某个会话里的图片附件
    /// 输出每条带 `attachment_id`（不透明 base64url 句柄），传给 `Extract` 时取回本体
    Attachments {
        chat: String,
        /// 仅增强列表的资源摘要与加密 DAT 大小；不解码或读取图片正文。
        #[serde(default, skip_serializing_if = "is_false")]
        image_metadata: bool,
        /// 类型过滤：当前仅支持 image
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kinds: Option<Vec<String>>,
        #[serde(default = "default_limit_50")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<i64>,
        #[serde(default, skip_serializing_if = "is_false")]
        with_meta: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        debug_source: bool,
    },
    /// 提取（解密）单个附件的本体到指定路径
    Extract {
        /// `Attachments` 返回的不透明 ID
        attachment_id: String,
        /// 写入的绝对路径（daemon 直接写盘，不经 socket 传 binary）
        output: String,
        /// 已存在时是否覆盖
        #[serde(default)]
        overwrite: bool,
    },
}

/// daemon 的响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub data: Value,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            error: None,
            data,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: Value::Null,
        }
    }

    pub fn to_json_line(&self) -> anyhow::Result<String> {
        let s = serde_json::to_string(self)?;
        Ok(s + "\n")
    }
}

fn default_limit_20() -> usize {
    20
}
fn default_limit_50() -> usize {
    50
}
fn default_limit_200() -> usize {
    200
}
fn is_false(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn voice_request_defaults_and_exact_roundtrip() {
        let request: Request = serde_json::from_value(json!({
            "cmd": "voice_messages", "chat": "synthetic-alice"
        }))
        .unwrap();
        assert!(matches!(
            &request,
            Request::VoiceMessages {
                limit: 20,
                offset: 0,
                since: None,
                until: None,
                ..
            }
        ));
        let explicit = json!({"cmd":"voice_messages","chat":"synthetic-alice",
            "limit":3,"offset":7,"since":0,"until":100});
        let request: Request = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), explicit);
        assert!(serde_json::from_value::<Request>(json!({
            "cmd":"voice_messages", "chat":"synthetic-alice", "offset":-1
        }))
        .is_err());
    }

    #[test]
    fn voice_response_keeps_flattened_object_contract() {
        let response = Response::ok(json!({"voices": [], "count": 0}));
        let line = response.to_json_line().unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"ok":true,"voices":[],"count":0})
        );
    }
}
