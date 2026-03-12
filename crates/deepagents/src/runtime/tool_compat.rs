use crate::provider::protocol::AgentToolCall;
use crate::runtime::ToolResultRecord;
use crate::types::{Message, ToolCall};

fn extract_error_fields(v: &serde_json::Value) -> (String, Option<String>, Option<String>) {
    match v {
        serde_json::Value::String(s) => (s.clone(), None, Some(s.clone())),
        serde_json::Value::Object(m) => {
            let code = m
                .get("code")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let msg = m
                .get("message")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let combined = match (&code, &msg) {
                (Some(c), Some(m)) => format!("{c}: {m}"),
                _ => v.to_string(),
            };
            (combined, code, msg)
        }
        _ => {
            let s = v.to_string();
            (s.clone(), None, Some(s))
        }
    }
}

fn extract_string<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = map.get(*k).and_then(|v| v.as_str()) {
            return Some(s);
        }
    }
    None
}

pub fn normalize_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    for mut msg in messages {
        if msg.role == "assistant" && msg.tool_calls.is_none() {
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(&msg.content)
            {
                if let Some(tc_val) = map.get("tool_calls") {
                    if let Ok(calls) = serde_json::from_value::<Vec<ToolCall>>(tc_val.clone()) {
                        msg.content = map
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if msg.reasoning_content.is_none() {
                            msg.reasoning_content = map
                                .get("reasoning_content")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string);
                        }
                        msg.tool_calls = Some(calls);
                    }
                }
            }
        }

        if msg.role == "tool" && msg.tool_call_id.is_none() {
            if let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(&msg.content)
            {
                if let Some(id) = extract_string(
                    &map,
                    &[
                        "tool_call_id",
                        "tool_use_id",
                        "toolUseId",
                        "call_id",
                        "id",
                        "tool_callid",
                    ],
                ) {
                    msg.tool_call_id = Some(id.to_string());
                }
                if msg.name.is_none() {
                    if let Some(name) = extract_string(&map, &["tool_name", "name"]) {
                        msg.name = Some(name.to_string());
                    }
                }
                if msg.status.is_none() {
                    if let Some(status) = extract_string(&map, &["status"]) {
                        msg.status = Some(status.to_string());
                    } else if map.get("error").is_some_and(|e| !e.is_null()) {
                        msg.status = Some("error".to_string());
                    } else {
                        msg.status = Some("success".to_string());
                    }
                }
            }
        }

        out.push(msg);
    }
    out
}

pub fn tool_results_from_messages(messages: &[Message]) -> Vec<ToolResultRecord> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.role != "tool" {
            continue;
        }
        let call_id = match msg.tool_call_id.as_deref() {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => continue,
        };

        let parsed = serde_json::from_str::<serde_json::Value>(&msg.content).ok();
        let (tool_name, output, error, error_code, error_message, status) = match parsed {
            Some(serde_json::Value::Object(map)) => {
                let tool_name = msg
                    .name
                    .clone()
                    .or_else(|| extract_string(&map, &["tool_name", "name"]).map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                let output = map
                    .get("output")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let (error, error_code, error_message) = map
                    .get("error")
                    .filter(|e| !e.is_null())
                    .map(extract_error_fields)
                    .map(|(e, c, m)| (Some(e), c, m))
                    .unwrap_or_else(|| (None, None, None));
                let error = error.or_else(|| {
                    if msg.status.as_deref() == Some("error") {
                        Some(msg.content.clone())
                    } else {
                        None
                    }
                });
                let status = msg
                    .status
                    .clone()
                    .or_else(|| extract_string(&map, &["status"]).map(|s| s.to_string()))
                    .or_else(|| {
                        if error.is_some() {
                            Some("error".to_string())
                        } else {
                            Some("success".to_string())
                        }
                    });
                (tool_name, output, error, error_code, error_message, status)
            }
            _ => {
                let tool_name = msg.name.clone().unwrap_or_else(|| "unknown".to_string());
                let status = msg.status.clone();
                let error = if status.as_deref() == Some("error") {
                    Some(msg.content.clone())
                } else {
                    None
                };
                (
                    tool_name,
                    serde_json::Value::Null,
                    error,
                    None,
                    None,
                    status,
                )
            }
        };

        out.push(ToolResultRecord {
            tool_name,
            call_id: Some(call_id),
            output,
            error,
            error_code,
            error_message,
            status,
        });
    }
    out
}

pub enum NormalizedToolCall {
    Valid(AgentToolCall),
    Invalid { call: AgentToolCall, error: String },
}

pub fn normalize_tool_call_for_execution(
    mut call: AgentToolCall,
    next_call_id: &mut u64,
) -> NormalizedToolCall {
    if call.tool_name.trim().is_empty() {
        call.tool_name = "unknown".to_string();
    }

    let need_id = call
        .call_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    if need_id {
        let id = format!("call-{}", *next_call_id);
        *next_call_id += 1;
        call.call_id = Some(id);
    }

    match &call.arguments {
        serde_json::Value::Object(_) => NormalizedToolCall::Valid(call),
        serde_json::Value::Null => {
            call.arguments = serde_json::json!({});
            NormalizedToolCall::Valid(call)
        }
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) if v.is_object() => {
                call.arguments = v;
                NormalizedToolCall::Valid(call)
            }
            Ok(_) => NormalizedToolCall::Invalid {
                call,
                error: "invalid_tool_call: arguments must be JSON object".to_string(),
            },
            Err(_) => NormalizedToolCall::Invalid {
                call,
                error: "invalid_tool_call: arguments must be JSON object string".to_string(),
            },
        },
        _ => NormalizedToolCall::Invalid {
            call,
            error: "invalid_tool_call: arguments must be object".to_string(),
        },
    }
}
