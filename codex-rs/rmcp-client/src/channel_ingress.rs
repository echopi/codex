//! MCP channel notification ingress: interception, parsing, and deduplication.
//!
//! When a trusted MCP server emits a custom notification matching one of the
//! channel ingress methods, this module validates the notification, deduplicates
//! by `server + source + msgId`, and forwards it via a caller-supplied callback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use codex_protocol::channel_notification::ChannelNotification;
use serde_json::Value;
use tracing::debug;
use tracing::warn;

const CHANNEL_METHOD: &str = "notifications/channel";
const CLAUDE_CHANNEL_METHOD: &str = "notifications/claude/channel";
const MESSAGE_METHOD: &str = "notifications/message";
const DEFAULT_DEDUPE_TTL: Duration = Duration::from_secs(3600);

/// Callback invoked when a validated, deduplicated channel notification is ready
/// for ingress into the active session.
pub type OnChannelNotification = Arc<dyn Fn(ChannelNotification) + Send + Sync>;

/// Tracks recently seen `(server, source, msgId)` triples to suppress duplicates.
#[derive(Clone)]
pub(crate) struct DedupeStore {
    seen: Arc<Mutex<HashMap<String, Instant>>>,
    ttl: Duration,
}

impl DedupeStore {
    pub(crate) fn new() -> Self {
        Self {
            seen: Arc::new(Mutex::new(HashMap::new())),
            ttl: DEFAULT_DEDUPE_TTL,
        }
    }

    /// Returns `true` if this key was already seen within the TTL window.
    pub(crate) fn is_duplicate(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        // Evict expired entries lazily on each check.
        map.retain(|_, ts| now.duration_since(*ts) < self.ttl);
        if map.contains_key(key) {
            true
        } else {
            map.insert(key.to_string(), now);
            false
        }
    }
}

/// Configuration for channel notification ingress on a single MCP server.
#[derive(Clone)]
pub struct ChannelIngressConfig {
    pub server_name: String,
    pub surface_notifications: bool,
    pub on_notification: OnChannelNotification,
}

/// Attempts to parse and forward a custom MCP server notification as a channel
/// ingress event. Returns `true` if the notification was handled (consumed).
pub(crate) fn try_handle_channel_notification(
    config: &ChannelIngressConfig,
    dedupe: &DedupeStore,
    method: &str,
    params: Option<&Value>,
) -> bool {
    if !config.surface_notifications {
        if is_channel_method(method) {
            debug!(
                server = %config.server_name,
                method,
                "channel notification ignored (surface_notifications=false)"
            );
        }
        return false;
    }

    let Some(params) = params else {
        return false;
    };

    let notification = match method {
        CHANNEL_METHOD | CLAUDE_CHANNEL_METHOD => {
            ChannelNotification::from_channel_params(&config.server_name, params)
        }
        MESSAGE_METHOD => {
            ChannelNotification::from_message_params(&config.server_name, params)
        }
        _ => return false,
    };

    let Some(notification) = notification else {
        warn!(
            server = %config.server_name,
            method,
            "failed to parse channel notification params"
        );
        return true;
    };

    if notification.is_oversized() {
        warn!(
            server = %config.server_name,
            msg_id = %notification.msg_id,
            "channel notification payload too large, dropping"
        );
        return true;
    }

    let key = notification.dedupe_key();
    if dedupe.is_duplicate(&key) {
        debug!(
            server = %config.server_name,
            msg_id = %notification.msg_id,
            "channel notification deduplicated"
        );
        return true;
    }

    debug!(
        server = %config.server_name,
        msg_id = %notification.msg_id,
        source = %notification.source,
        "forwarding channel notification to active session"
    );
    (config.on_notification)(notification);
    true
}

fn is_channel_method(method: &str) -> bool {
    matches!(
        method,
        CHANNEL_METHOD | CLAUDE_CHANNEL_METHOD | MESSAGE_METHOD
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    fn test_config(
        surface: bool,
        counter: Arc<AtomicUsize>,
    ) -> ChannelIngressConfig {
        ChannelIngressConfig {
            server_name: "test-server".into(),
            surface_notifications: surface,
            on_notification: Arc::new(move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
        }
    }

    #[test]
    fn surface_off_ignores_channel_notification() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(false, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hello"
        });
        let handled = try_handle_channel_notification(
            &config,
            &dedupe,
            CHANNEL_METHOD,
            Some(&params),
        );
        assert!(!handled);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn surface_on_forwards_channel_notification() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hello"
        });
        let handled = try_handle_channel_notification(
            &config,
            &dedupe,
            CHANNEL_METHOD,
            Some(&params),
        );
        assert!(handled);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn claude_channel_method_also_handled() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hi"
        });
        assert!(try_handle_channel_notification(
            &config,
            &dedupe,
            CLAUDE_CHANNEL_METHOD,
            Some(&params),
        ));
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn message_method_requires_to_session() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();

        // Without toSession
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hi"
        });
        try_handle_channel_notification(
            &config,
            &dedupe,
            MESSAGE_METHOD,
            Some(&params),
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);

        // With toSession: true
        let params = json!({
            "toSession": true,
            "source": "dingtalk",
            "msgId": "m2",
            "content": "hi"
        });
        try_handle_channel_notification(
            &config,
            &dedupe,
            MESSAGE_METHOD,
            Some(&params),
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn duplicate_msg_id_deduplicated() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "hello"
        });

        // First call: forwarded
        try_handle_channel_notification(
            &config,
            &dedupe,
            CHANNEL_METHOD,
            Some(&params),
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Second call with same msgId: deduplicated
        try_handle_channel_notification(
            &config,
            &dedupe,
            CHANNEL_METHOD,
            Some(&params),
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn oversized_payload_rejected() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({
            "source": "dingtalk",
            "msgId": "m1",
            "content": "x".repeat(16385)
        });
        let handled = try_handle_channel_notification(
            &config,
            &dedupe,
            CHANNEL_METHOD,
            Some(&params),
        );
        assert!(handled);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unrelated_method_ignored() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = test_config(true, counter.clone());
        let dedupe = DedupeStore::new();
        let params = json!({"source": "x", "msgId": "m", "content": "y"});
        let handled = try_handle_channel_notification(
            &config,
            &dedupe,
            "notifications/progress",
            Some(&params),
        );
        assert!(!handled);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
