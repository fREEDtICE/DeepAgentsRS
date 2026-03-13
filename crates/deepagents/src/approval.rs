use std::collections::HashSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    NonInteractive,
    Interactive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub command: String,
    pub root: String,
    pub mode: ExecutionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow { reason: String },
    Deny { code: String, reason: String },
    RequireApproval { code: String, reason: String },
}

pub trait ApprovalPolicy: Send + Sync {
    fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision;
}

#[derive(Clone)]
pub struct DefaultApprovalPolicy {
    allow: HashSet<String>,
}

impl DefaultApprovalPolicy {
    pub fn new(allow_list: impl IntoIterator<Item = String>) -> Self {
        let allow = allow_list
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<HashSet<_>>();
        Self { allow }
    }
}

impl ApprovalPolicy for DefaultApprovalPolicy {
    fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
        let cmd = req.command.trim();
        if cmd.is_empty() {
            return ApprovalDecision::Deny {
                code: "empty_command".to_string(),
                reason: "empty command".to_string(),
            };
        }

        if contains_control_chars(cmd)
            || contains_dangerous_patterns(cmd)
            || contains_standalone_ampersand(cmd)
        {
            return ApprovalDecision::Deny {
                code: "dangerous_pattern".to_string(),
                reason: "command contains dangerous patterns".to_string(),
            };
        }

        if self.allow.is_empty() {
            return ApprovalDecision::RequireApproval {
                code: "approval_required".to_string(),
                reason: "allow-list is empty".to_string(),
            };
        }

        let segments = split_shell_segments(cmd);
        if segments.iter().any(|s| s.trim().is_empty()) {
            return ApprovalDecision::Deny {
                code: "empty_command".to_string(),
                reason: "empty command segment".to_string(),
            };
        }

        for seg in segments {
            let Some(prog) = first_program_token(seg.as_str()) else {
                return ApprovalDecision::Deny {
                    code: "empty_command".to_string(),
                    reason: "empty command segment".to_string(),
                };
            };
            if !self.allow.contains(&prog) {
                return ApprovalDecision::Deny {
                    code: "not_in_allow_list".to_string(),
                    reason: format!("program not in allow-list: {}", prog),
                };
            }
        }

        ApprovalDecision::Allow {
            reason: "allow-list matched".to_string(),
        }
    }
}

pub fn redact_command(command: &str) -> String {
    let tokens = shell_like_split(command);
    if tokens.is_empty() {
        return String::new();
    }

    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut redact_next = false;

    for t in tokens {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }

        let lower = t.to_ascii_lowercase();
        if lower == "--token" || lower == "--key" || lower == "--password" || lower == "--secret" {
            out.push(t);
            redact_next = true;
            continue;
        }

        if let Some((k, _v)) = t.split_once('=') {
            if is_secret_key_name(k) {
                out.push(format!("{}=***", k));
                continue;
            }
        }

        if lower.starts_with("--token=")
            || lower.starts_with("--key=")
            || lower.starts_with("--password=")
            || lower.starts_with("--secret=")
        {
            let (k, _) = t.split_once('=').unwrap_or((&t, ""));
            out.push(format!("{}=***", k));
            continue;
        }

        out.push(t);
    }

    out.join(" ")
}

fn is_secret_key_name(k: &str) -> bool {
    let upper = k.to_ascii_uppercase();
    upper == "TOKEN" || upper == "KEY" || upper == "PASSWORD" || upper == "SECRET"
}

fn contains_control_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\n' | '\r' | '\t' | '\0'))
}

fn contains_dangerous_patterns(s: &str) -> bool {
    let needles = [
        "$(", "`", "${", "$'", "<<<", ">>", "<<", ">>", ">", "<", "<(", ">(",
    ];
    if needles.iter().any(|n| s.contains(n)) {
        return true;
    }
    let re = Regex::new(r"\$[A-Za-z_][A-Za-z0-9_]*").ok();
    if let Some(re) = re {
        if re.is_match(s) {
            return true;
        }
    }
    false
}

fn contains_standalone_ampersand(s: &str) -> bool {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'&' {
            let prev_is_amp = i > 0 && bytes[i - 1] == b'&';
            let next_is_amp = i + 1 < bytes.len() && bytes[i + 1] == b'&';
            if !prev_is_amp && !next_is_amp {
                return true;
            }
        }
    }
    false
}

fn split_shell_segments(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                buf.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                buf.push(c);
            }
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    let _ = chars.next();
                    out.push(buf.trim().to_string());
                    buf.clear();
                } else {
                    buf.push(c);
                }
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    let _ = chars.next();
                    out.push(buf.trim().to_string());
                    buf.clear();
                } else {
                    out.push(buf.trim().to_string());
                    buf.clear();
                }
            }
            ';' if !in_single && !in_double => {
                out.push(buf.trim().to_string());
                buf.clear();
            }
            _ => buf.push(c),
        }
    }

    if !buf.trim().is_empty() || command.ends_with(';') {
        out.push(buf.trim().to_string());
    }

    out
}

fn first_program_token(segment: &str) -> Option<String> {
    let mut tokens = shell_like_split(segment);
    while let Some(t) = tokens.first() {
        if is_env_assignment_token(t) {
            let _ = tokens.remove(0);
            continue;
        }
        break;
    }
    tokens
        .first()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn is_env_assignment_token(t: &str) -> bool {
    let Some((k, _v)) = t.split_once('=') else {
        return false;
    };
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn shell_like_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;

    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\n' | '\r' | '\t' if !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(cur.clone());
                    cur.clear();
                }
                while matches!(chars.peek(), Some(' ' | '\n' | '\r' | '\t')) {
                    let _ = chars.next();
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
