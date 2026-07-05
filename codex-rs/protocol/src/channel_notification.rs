//! Types for MCP channel notification ingress into active sessions.

use serde::Deserialize;
use serde::Serialize;

const MAX_PAYLOAD_BYTES: usize = 16384;

/// Parsed channel notification envelope from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelNotification {
    pub server_name: String,
    pub source: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub conversation_type: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    pub msg_id: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub content: String,
}

impl ChannelNotification {
    /// Format the notification as an XML block that preserves source metadata
    /// and is model-readable.
    pub fn to_xml(&self) -> String {
        let mut attrs = format!(r#"source="{}""#, xml_escape(&self.source));
        if let Some(ref cid) = self.conversation_id {
            attrs.push_str(&format!(r#" conversation_id="{}""#, xml_escape(cid)));
        }
        if let Some(ref ct) = self.conversation_type {
            attrs.push_str(&format!(r#" conversation_type="{}""#, xml_escape(ct)));
        }
        if let Some(ref sender) = self.sender {
            attrs.push_str(&format!(r#" sender="{}""#, xml_escape(sender)));
        }
        if let Some(ref ts) = self.timestamp {
            attrs.push_str(&format!(r#" timestamp="{}""#, xml_escape(ts)));
        }
        attrs.push_str(&format!(r#" msg_id="{}""#, xml_escape(&self.msg_id)));
        format!("<channel {}>\n{}\n</channel>", attrs, &self.content)
    }

    /// Dedupe key combining server identity, notification source, and message id.
    pub fn dedupe_key(&self) -> String {
        format!("{}:{}:{}", self.server_name, self.source, self.msg_id)
    }

    /// Returns `true` when the content exceeds the maximum payload size.
    pub fn is_oversized(&self) -> bool {
        self.content.len() > MAX_PAYLOAD_BYTES
    }

    /// Parse from an MCP `notifications/channel` or `notifications/claude/channel`
    /// JSON params object.
    pub fn from_channel_params(
        server_name: &str,
        params: &serde_json::Value,
    ) -> Option<Self> {
        let obj = params.as_object()?;
        let source = obj.get("source")?.as_str()?.to_string();
        let msg_id = obj.get("msgId")?.as_str()?.to_string();
        let content = obj.get("content")?.as_str()?.to_string();
        Some(Self {
            server_name: server_name.to_string(),
            source,
            conversation_id: obj.get("conversationId").and_then(|v| v.as_str()).map(String::from),
            conversation_type: obj.get("conversationType").and_then(|v| v.as_str()).map(String::from),
            sender: obj.get("sender").and_then(|v| v.as_str()).map(String::from),
            sender_id: obj.get("senderId").and_then(|v| v.as_str()).map(String::from),
            msg_id,
            timestamp: obj.get("timestamp").and_then(|v| v.as_str()).map(String::from),
            content,
        })
    }

    /// Parse from an MCP `notifications/message` JSON params object.
    /// Returns `None` if `toSession` is not `true`.
    pub fn from_message_params(
        server_name: &str,
        params: &serde_json::Value,
    ) -> Option<Self> {
        let obj = params.as_object()?;
        if !obj.get("toSession").and_then(|v| v.as_bool()).unwrap_or(false) {
            return None;
        }
        Self::from_channel_params(server_name, params)
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_xml_produces_expected_output() {
        let n = ChannelNotification {
            server_name: "dingtalk".into(),
            source: "dingtalk".into(),
            conversation_id: Some("cid123".into()),
            conversation_type: Some("direct".into()),
            sender: Some("Alice".into()),
            sender_id: None,
            msg_id: "msg-1".into(),
            timestamp: Some("2026-07-05T08:00:00.000Z".into()),
            content: "hello".into(),
        };
        let xml = n.to_xml();
        assert!(xml.starts_with("<channel "));
        assert!(xml.contains(r#"source="dingtalk""#));
        assert!(xml.contains(r#"msg_id="msg-1""#));
        assert!(xml.contains("hello"));
        assert!(xml.ends_with("</channel>"));
    }

    #[test]
    fn from_channel_params_parses_valid_envelope() {
        let params = json!({
            "source": "dingtalk",
            "conversationId": "cid",
            "conversationType": "group",
            "sender": "Bob",
            "senderId": "uid-1",
            "msgId": "msg-42",
            "timestamp": "2026-07-05T10:00:00Z",
            "content": "test message"
        });
        let n = ChannelNotification::from_channel_params("dt", &params).unwrap();
        assert_eq!(n.server_name, "dt");
        assert_eq!(n.source, "dingtalk");
        assert_eq!(n.msg_id, "msg-42");
        assert_eq!(n.content, "test message");
        assert_eq!(n.sender.as_deref(), Some("Bob"));
    }

    #[test]
    fn from_channel_params_returns_none_on_missing_fields() {
        let params = json!({"source": "x"});
        assert!(ChannelNotification::from_channel_params("s", &params).is_none());
    }

    #[test]
    fn from_message_params_requires_to_session() {
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hi"
        });
        assert!(ChannelNotification::from_message_params("s", &params).is_none());

        let params_with_flag = json!({
            "toSession": true,
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hi"
        });
        assert!(ChannelNotification::from_message_params("s", &params_with_flag).is_some());
    }

    #[test]
    fn from_message_params_rejects_false_to_session() {
        let params = json!({
            "toSession": false,
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hi"
        });
        assert!(ChannelNotification::from_message_params("s", &params).is_none());
    }

    #[test]
    fn dedupe_key_combines_server_source_msgid() {
        let n = ChannelNotification {
            server_name: "srv".into(),
            source: "src".into(),
            conversation_id: None,
            conversation_type: None,
            sender: None,
            sender_id: None,
            msg_id: "id1".into(),
            timestamp: None,
            content: "x".into(),
        };
        assert_eq!(n.dedupe_key(), "srv:src:id1");
    }

    #[test]
    fn oversized_payload_detected() {
        let n = ChannelNotification {
            server_name: "s".into(),
            source: "s".into(),
            conversation_id: None,
            conversation_type: None,
            sender: None,
            sender_id: None,
            msg_id: "m".into(),
            timestamp: None,
            content: "x".repeat(MAX_PAYLOAD_BYTES + 1),
        };
        assert!(n.is_oversized());
    }
}
