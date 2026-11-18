//! The conversation messages Claude Code writes on stdout.
//! Only what seimei reads is typed; the rest stays JSON, so a field Claude Code adds or drops does not end the session.

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AssistantBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AssistantMessage {
    pub message: AssistantBody,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Clone, Debug, Deserialize)]
pub struct UserBody {
    pub content: UserContent,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UserMessage {
    pub message: UserBody,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
}

/// init, compact_boundary, task_started, task_notification, ...: the subtype and the whole message.
#[derive(Clone, Debug)]
pub struct SystemMessage {
    pub subtype: String,
    pub data: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResultMessage {
    pub subtype: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub duration_api_ms: u64,
    pub is_error: bool,
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Value>,
    /// Per model, this session's totals so far (inputTokens, outputTokens, cacheReadInputTokens, ...).
    #[serde(default, rename = "modelUsage")]
    pub model_usage: Option<Value>,
    /// Why the turn ended: "completed", "aborted_streaming", "api_error", ...
    #[serde(default)]
    pub terminal_reason: Option<String>,
}

impl ResultMessage {
    /// The turn was cut off: by an interrupt, or by a priority "now" message that the next turn answers.
    /// Claude Code ends a turn cut by "now" as a success with no text; older versions ended it as error_during_execution.
    pub fn aborted(&self) -> bool {
        matches!(self.terminal_reason.as_deref(), Some("aborted_streaming" | "aborted_tools"))
            || self.is_error && self.subtype == "error_during_execution"
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct StreamEvent {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub session_id: String,
    pub event: Value,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Assistant(AssistantMessage),
    User(UserMessage),
    System(SystemMessage),
    Result(ResultMessage),
    StreamEvent(StreamEvent),
    /// rate_limit_event's rate_limit_info: status, resetsAt, rateLimitType, utilization.
    RateLimit(Value),
    /// A type this client does not know: passed on as it came.
    Other(Value),
    /// A line that could not be read as a message (bad JSON, an unexpected shape, longer than Options::max_line).
    /// The session goes on.
    Unreadable {
        error: String,
        line: String,
    },
}

impl Message {
    /// A parsed stdout line that is not a control message.
    pub(crate) fn from_value(v: Value) -> Message {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let typed = match kind.as_str() {
            "assistant" => serde_json::from_value(v.clone()).map(Message::Assistant),
            "user" => serde_json::from_value(v.clone()).map(Message::User),
            "result" => serde_json::from_value(v.clone()).map(Message::Result),
            "stream_event" => serde_json::from_value(v.clone()).map(Message::StreamEvent),
            "system" => {
                let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("").to_string();
                return Message::System(SystemMessage { subtype, data: v });
            }
            "rate_limit_event" => return Message::RateLimit(v.get("rate_limit_info").cloned().unwrap_or(Value::Null)),
            _ => return Message::Other(v),
        };
        typed.unwrap_or_else(|e| Message::Unreadable { error: format!("{kind}: {e}"), line: short(&v.to_string()) })
    }
}

pub(crate) fn short(s: &str) -> String {
    let mut end = s.len().min(300);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_an_assistant_turn() {
        let m = Message::from_value(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [
            {"type": "text", "text": "hi"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            {"type": "server_tool_use", "id": "x"}]}, "parent_tool_use_id": null}));
        let Message::Assistant(a) = m else { panic!("{m:?}") };
        assert_eq!(a.message.content[0], ContentBlock::Text { text: "hi".into() });
        assert!(matches!(&a.message.content[1], ContentBlock::ToolUse { name, .. } if name == "Bash"));
        assert_eq!(a.message.content[2], ContentBlock::Other);
    }

    #[test]
    fn user_content_is_text_or_blocks() {
        let Message::User(u) =
            Message::from_value(json!({"type": "user", "message": {"role": "user", "content": "hey"}}))
        else {
            panic!()
        };
        assert!(matches!(u.message.content, UserContent::Text(t) if t == "hey"));
        let Message::User(u) = Message::from_value(json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok", "is_error": false}]}}))
        else {
            panic!()
        };
        assert!(
            matches!(&u.message.content, UserContent::Blocks(b) if matches!(&b[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t1"))
        );
    }

    #[test]
    fn a_result_keeps_model_usage() {
        let Message::Result(r) = Message::from_value(json!({"type": "result", "subtype": "success", "is_error": false,
            "session_id": "s", "result": "done", "modelUsage": {"claude-opus-5-5": {"inputTokens": 3}}}))
        else {
            panic!()
        };
        assert_eq!(r.model_usage.unwrap()["claude-opus-5-5"]["inputTokens"], 3);
    }

    #[test]
    fn drift_does_not_end_the_session() {
        assert!(matches!(Message::from_value(json!({"type": "result"})), Message::Unreadable { .. }));
        assert!(matches!(Message::from_value(json!({"type": "brand_new"})), Message::Other(_)));
        assert!(
            matches!(Message::from_value(json!({"type": "system", "subtype": "init"})), Message::System(s) if s.subtype == "init")
        );
    }
}
