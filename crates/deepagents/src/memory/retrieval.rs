use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::access::filter_readable_entries;
use crate::memory::identity::ResolvedMemoryActor;
use crate::memory::protocol::{
    MemoryEntry, MemoryError, MemoryQuery, MemoryScopeType, MemoryStore, MemoryType,
};
use crate::types::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryIntent {
    Personal,
    Workspace,
    Mixed,
    MemorizeCommand,
    FactualRecall,
    PlanningTask,
    PreferenceSensitive,
    #[default]
    General,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalQueryPlan {
    pub name: String,
    pub text: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryRetrievalActorView {
    pub user_id: String,
    pub thread_id: String,
    pub workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySelectionDiagnostic {
    pub key: String,
    pub memory_id: String,
    pub scope_type: MemoryScopeType,
    pub scope_id: String,
    pub memory_type: MemoryType,
    pub section: String,
    pub score: f32,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub matched_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryRetrievalDiagnostics {
    pub mode: String,
    pub actor: MemoryRetrievalActorView,
    pub intent: MemoryIntent,
    #[serde(default)]
    pub queries: Vec<RetrievalQueryPlan>,
    #[serde(default)]
    pub phase_counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub selected: Vec<MemorySelectionDiagnostic>,
    #[serde(default)]
    pub rendered_sections: Vec<String>,
    pub truncated: bool,
    pub injected_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPackItem {
    pub key: String,
    pub title: String,
    pub value: String,
    pub scope_type: MemoryScopeType,
    pub scope_id: String,
    pub memory_type: MemoryType,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPackSection {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub entries: Vec<MemoryPackItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryPack {
    pub intent: MemoryIntent,
    #[serde(default)]
    pub sections: Vec<MemoryPackSection>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct PlannedQuery {
    name: String,
    text: String,
    weight: f32,
    tokens: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct RankedEntry {
    entry: MemoryEntry,
    score: f32,
    reasons: Vec<String>,
    matched_queries: Vec<String>,
}

pub async fn build_memory_pack<S: MemoryStore>(
    store: &S,
    actor: &ResolvedMemoryActor,
    messages: &[Message],
    max_chars: usize,
) -> Result<(MemoryPack, MemoryRetrievalDiagnostics, String), MemoryError> {
    let prompt = latest_user_message(messages);
    let intent = classify_intent(prompt);
    let queries = planned_queries(prompt, intent, !actor.workspace_ids.is_empty());

    let mut phase_counts = BTreeMap::new();
    let mut gathered: Vec<(&'static str, Vec<MemoryEntry>)> = Vec::new();

    let thread_recent = query_scope(
        store,
        actor,
        MemoryScopeType::Thread,
        &actor.thread_id,
        None,
        Some(12),
    )
    .await?;
    phase_counts.insert("recent_thread".to_string(), thread_recent.len());
    gathered.push(("recent_thread", thread_recent));

    let mut pinned_explicit = query_scope(
        store,
        actor,
        MemoryScopeType::Thread,
        &actor.thread_id,
        Some(true),
        Some(8),
    )
    .await?;
    pinned_explicit.extend(
        query_scope(
            store,
            actor,
            MemoryScopeType::User,
            &actor.user_id,
            Some(true),
            Some(8),
        )
        .await?,
    );
    for workspace_id in &actor.workspace_ids {
        pinned_explicit.extend(
            query_scope(
                store,
                actor,
                MemoryScopeType::Workspace,
                workspace_id,
                Some(true),
                Some(8),
            )
            .await?,
        );
    }
    phase_counts.insert("pinned_explicit".to_string(), pinned_explicit.len());
    gathered.push(("pinned_explicit", pinned_explicit));

    let user_scope = query_scope(
        store,
        actor,
        MemoryScopeType::User,
        &actor.user_id,
        None,
        Some(20),
    )
    .await?;
    phase_counts.insert("user_scope".to_string(), user_scope.len());
    gathered.push(("user_scope", user_scope));

    let mut workspace_scope = Vec::new();
    for workspace_id in &actor.workspace_ids {
        workspace_scope.extend(
            query_scope(
                store,
                actor,
                MemoryScopeType::Workspace,
                workspace_id,
                None,
                Some(20),
            )
            .await?,
        );
    }
    phase_counts.insert("workspace_scope".to_string(), workspace_scope.len());
    gathered.push(("workspace_scope", workspace_scope));

    let mut ranked: BTreeMap<String, RankedEntry> = BTreeMap::new();
    for (phase, entries) in gathered {
        for entry in entries {
            let dedupe_key = if entry.memory_id.is_empty() {
                entry.key.clone()
            } else {
                entry.memory_id.clone()
            };
            let (score, reasons, matched_queries) = score_entry(&entry, &queries, intent, phase);
            let candidate = RankedEntry {
                entry,
                score,
                reasons,
                matched_queries,
            };
            let should_replace = ranked
                .get(&dedupe_key)
                .map(|existing| candidate.score > existing.score)
                .unwrap_or(true);
            if should_replace {
                ranked.insert(dedupe_key, candidate);
            }
        }
    }

    let mut ordered = ranked.into_values().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.score.total_cmp(&a.score));

    let mut warnings = Vec::new();
    if matches!(intent, MemoryIntent::Workspace | MemoryIntent::Mixed)
        && actor.workspace_ids.is_empty()
    {
        warnings.push(
            "Workspace-oriented request detected, but no workspace membership was resolved."
                .to_string(),
        );
    }
    if ordered.is_empty() {
        warnings.push("No scoped memory matched this request.".to_string());
    }

    let (pack, selected, truncated, injected_chars) =
        assemble_pack(ordered, intent, warnings.clone(), max_chars);

    let diagnostics = MemoryRetrievalDiagnostics {
        mode: "scoped".to_string(),
        actor: MemoryRetrievalActorView {
            user_id: actor.user_id.clone(),
            thread_id: actor.thread_id.clone(),
            workspace_ids: actor.workspace_ids.iter().cloned().collect(),
        },
        intent,
        queries: queries
            .iter()
            .map(|query| RetrievalQueryPlan {
                name: query.name.clone(),
                text: query.text.clone(),
                weight: query.weight,
            })
            .collect(),
        phase_counts,
        rendered_sections: pack
            .sections
            .iter()
            .map(|section| section.name.clone())
            .collect(),
        selected,
        truncated,
        injected_chars,
    };

    let rendered = render_memory_pack(&pack, &diagnostics);
    Ok((pack, diagnostics, rendered))
}

fn latest_user_message(messages: &[Message]) -> &str {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("")
}

fn classify_intent(message: &str) -> MemoryIntent {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("remember ") || normalized.contains("remember that") {
        return MemoryIntent::MemorizeCommand;
    }
    if contains_any(
        &normalized,
        &[
            "prefer",
            "preference",
            "style",
            "tone",
            "language",
            "concise",
        ],
    ) {
        return MemoryIntent::PreferenceSensitive;
    }
    if contains_any(
        &normalized,
        &["plan", "todo", "task", "deadline", "next step", "roadmap"],
    ) {
        return MemoryIntent::PlanningTask;
    }
    let personal = contains_any(
        &normalized,
        &["i ", "my ", "me ", "myself", "personal", "prefer"],
    );
    let workspace = contains_any(
        &normalized,
        &[
            "team",
            "workspace",
            "project",
            "release",
            "repo",
            "sprint",
            "our ",
        ],
    );
    match (personal, workspace) {
        (true, true) => MemoryIntent::Mixed,
        (true, false) => MemoryIntent::Personal,
        (false, true) => MemoryIntent::Workspace,
        (false, false) if normalized.ends_with('?') => MemoryIntent::FactualRecall,
        _ => MemoryIntent::General,
    }
}

fn contains_any(input: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| input.contains(needle))
}

fn planned_queries(message: &str, intent: MemoryIntent, has_workspace: bool) -> Vec<PlannedQuery> {
    let mut queries = Vec::new();
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        queries.push(PlannedQuery {
            name: "direct".to_string(),
            text: trimmed.to_string(),
            weight: 1.0,
            tokens: tokenize(trimmed),
        });
    }

    if matches!(
        intent,
        MemoryIntent::PreferenceSensitive
            | MemoryIntent::Personal
            | MemoryIntent::Mixed
            | MemoryIntent::MemorizeCommand
    ) {
        queries.push(PlannedQuery {
            name: "preferences".to_string(),
            text: "preferences reply style language tone".to_string(),
            weight: 0.75,
            tokens: tokenize("preferences reply style language tone"),
        });
    }

    if matches!(intent, MemoryIntent::PlanningTask | MemoryIntent::Mixed) {
        queries.push(PlannedQuery {
            name: "active_goals".to_string(),
            text: "active goals tasks deadlines next steps".to_string(),
            weight: 0.6,
            tokens: tokenize("active goals tasks deadlines next steps"),
        });
    }

    if has_workspace && matches!(intent, MemoryIntent::Workspace | MemoryIntent::Mixed) {
        queries.push(PlannedQuery {
            name: "workspace_context".to_string(),
            text: "workspace project team release conventions decisions".to_string(),
            weight: 0.7,
            tokens: tokenize("workspace project team release conventions decisions"),
        });
    }

    queries
}

async fn query_scope<S: MemoryStore>(
    store: &S,
    actor: &ResolvedMemoryActor,
    scope_type: MemoryScopeType,
    scope_id: &str,
    pinned: Option<bool>,
    limit: Option<usize>,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let entries = store
        .query(MemoryQuery {
            scope_type: Some(scope_type),
            scope_id: Some(scope_id.to_string()),
            pinned,
            limit,
            ..Default::default()
        })
        .await?;
    Ok(filter_readable_entries(entries, actor))
}

fn score_entry(
    entry: &MemoryEntry,
    queries: &[PlannedQuery],
    intent: MemoryIntent,
    phase: &str,
) -> (f32, Vec<String>, Vec<String>) {
    let entry_tokens = entry_tokens(entry);
    let mut semantic = 0.0f32;
    let mut matched_queries = Vec::new();
    for query in queries {
        if query.tokens.is_empty() {
            continue;
        }
        let overlap = overlap_score(&entry_tokens, &query.tokens);
        if overlap > 0.0 {
            matched_queries.push(query.name.clone());
        }
        semantic = semantic.max(overlap * query.weight);
    }

    let recency = recency_score(&entry.updated_at);
    let salience = entry.salience.clamp(0.0, 1.0);
    let confidence = entry.confidence.clamp(0.0, 1.0);
    let scope_bonus = scope_bonus(entry.scope_type, intent);
    let type_bonus = type_bonus(entry, intent);
    let pinned_bonus = if entry.pinned || entry.memory_type == MemoryType::Pinned {
        0.35
    } else {
        0.0
    };
    let phase_bonus = if phase == "recent_thread" { 0.08 } else { 0.0 };

    let score = semantic * 0.45
        + recency * 0.10
        + salience * 0.15
        + confidence * 0.10
        + scope_bonus
        + type_bonus
        + pinned_bonus
        + phase_bonus;

    let reasons = vec![
        format!("semantic={semantic:.2}"),
        format!("recency={recency:.2}"),
        format!("scope_bonus={scope_bonus:.2}"),
        format!("type_bonus={type_bonus:.2}"),
        format!("confidence={confidence:.2}"),
        format!("salience={salience:.2}"),
        format!("phase={phase}"),
    ];
    (score, reasons, matched_queries)
}

fn entry_tokens(entry: &MemoryEntry) -> BTreeSet<String> {
    let mut tokens = tokenize(&entry.title);
    tokens.extend(tokenize(&entry.value));
    tokens.extend(tokenize(&entry.key));
    tokens.extend(entry.tags.iter().flat_map(|tag| tokenize(tag)));
    tokens.extend(tokenize(memory_type_label(entry.memory_type)));
    tokens
}

fn tokenize(input: &str) -> BTreeSet<String> {
    input
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn overlap_score(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let overlap = left.intersection(right).count() as f32;
    overlap / (right.len() as f32)
}

fn recency_score(timestamp: &str) -> f32 {
    let parsed = DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .ok();
    let Some(parsed) = parsed else {
        return 0.3;
    };
    let age_hours = (Utc::now() - parsed).num_hours().max(0) as f32;
    (1.0 / (1.0 + (age_hours / 168.0))).clamp(0.0, 1.0)
}

fn scope_bonus(scope: MemoryScopeType, intent: MemoryIntent) -> f32 {
    match intent {
        MemoryIntent::Workspace | MemoryIntent::PlanningTask => match scope {
            MemoryScopeType::Thread => 0.22,
            MemoryScopeType::Workspace => 0.25,
            MemoryScopeType::User => 0.12,
            MemoryScopeType::System => 0.0,
        },
        MemoryIntent::PreferenceSensitive
        | MemoryIntent::Personal
        | MemoryIntent::MemorizeCommand => match scope {
            MemoryScopeType::Thread => 0.25,
            MemoryScopeType::User => 0.22,
            MemoryScopeType::Workspace => 0.10,
            MemoryScopeType::System => 0.0,
        },
        MemoryIntent::Mixed | MemoryIntent::FactualRecall | MemoryIntent::General => match scope {
            MemoryScopeType::Thread => 0.24,
            MemoryScopeType::User => 0.18,
            MemoryScopeType::Workspace => 0.16,
            MemoryScopeType::System => 0.0,
        },
    }
}

fn type_bonus(entry: &MemoryEntry, intent: MemoryIntent) -> f32 {
    match intent {
        MemoryIntent::PreferenceSensitive
        | MemoryIntent::Personal
        | MemoryIntent::MemorizeCommand => match entry.memory_type {
            MemoryType::Procedural | MemoryType::Pinned => 0.10,
            MemoryType::Profile => 0.08,
            MemoryType::Semantic => 0.05,
            MemoryType::Episodic => 0.02,
        },
        MemoryIntent::PlanningTask | MemoryIntent::Workspace => match entry.memory_type {
            MemoryType::Procedural => 0.10,
            MemoryType::Semantic | MemoryType::Pinned => 0.07,
            MemoryType::Episodic => 0.04,
            MemoryType::Profile => 0.02,
        },
        MemoryIntent::Mixed | MemoryIntent::FactualRecall | MemoryIntent::General => {
            match entry.memory_type {
                MemoryType::Pinned => 0.09,
                MemoryType::Semantic => 0.07,
                MemoryType::Profile => 0.06,
                MemoryType::Procedural => 0.05,
                MemoryType::Episodic => 0.03,
            }
        }
    }
}

fn assemble_pack(
    ordered: Vec<RankedEntry>,
    intent: MemoryIntent,
    warnings: Vec<String>,
    max_chars: usize,
) -> (MemoryPack, Vec<MemorySelectionDiagnostic>, bool, usize) {
    let mut buckets: BTreeMap<&'static str, Vec<RankedEntry>> = BTreeMap::new();
    for candidate in ordered {
        buckets
            .entry(section_name(&candidate.entry))
            .or_default()
            .push(candidate);
    }

    let ordered_sections = [
        ("thread_memory", "Thread Memory"),
        ("pinned_memory", "Pinned Memory"),
        ("user_preferences", "User Preferences"),
        ("user_profile", "User Profile"),
        ("active_goals", "Active Goals"),
        ("workspace_context", "Workspace Context"),
        ("relevant_episodic", "Relevant Episodic"),
    ];

    let mut pack = MemoryPack {
        intent,
        sections: Vec::new(),
        warnings,
    };
    let mut diagnostics = Vec::new();
    let mut injected_chars = 0usize;
    let mut truncated = false;

    for (section_key, section_title) in ordered_sections {
        let Some(entries) = buckets.get(section_key) else {
            continue;
        };

        let mut rendered_entries = Vec::new();
        for candidate in entries {
            let item = MemoryPackItem {
                key: candidate.entry.key.clone(),
                title: display_title(&candidate.entry),
                value: condensed_value(&candidate.entry.value),
                scope_type: candidate.entry.scope_type,
                scope_id: candidate.entry.scope_id.clone(),
                memory_type: candidate.entry.memory_type,
                pinned: candidate.entry.pinned,
            };
            let line = render_item_line(&item);
            if injected_chars + section_title.len() + line.len() + 8 > max_chars {
                truncated = true;
                continue;
            }
            injected_chars += line.len();
            rendered_entries.push(item);
            diagnostics.push(MemorySelectionDiagnostic {
                key: candidate.entry.key.clone(),
                memory_id: candidate.entry.memory_id.clone(),
                scope_type: candidate.entry.scope_type,
                scope_id: candidate.entry.scope_id.clone(),
                memory_type: candidate.entry.memory_type,
                section: section_key.to_string(),
                score: candidate.score,
                reasons: candidate.reasons.clone(),
                matched_queries: candidate.matched_queries.clone(),
            });
        }

        if !rendered_entries.is_empty() {
            pack.sections.push(MemoryPackSection {
                name: section_key.to_string(),
                title: section_title.to_string(),
                entries: rendered_entries,
            });
        }
    }

    if pack.sections.is_empty() && pack.warnings.is_empty() {
        pack.warnings
            .push("No scoped memory matched this request.".to_string());
    }

    let warning_chars: usize = pack.warnings.iter().map(|warning| warning.len() + 4).sum();
    if injected_chars + warning_chars > max_chars && !pack.warnings.is_empty() {
        truncated = true;
        pack.warnings.truncate(1);
    } else {
        injected_chars += warning_chars;
    }

    (pack, diagnostics, truncated, injected_chars)
}

fn section_name(entry: &MemoryEntry) -> &'static str {
    if entry.scope_type == MemoryScopeType::Thread {
        return "thread_memory";
    }
    if entry.pinned || entry.memory_type == MemoryType::Pinned {
        return "pinned_memory";
    }
    if entry.scope_type == MemoryScopeType::Workspace {
        return "workspace_context";
    }
    if has_goal_signal(entry) {
        return "active_goals";
    }
    if entry.memory_type == MemoryType::Episodic {
        return "relevant_episodic";
    }
    if has_preference_signal(entry) {
        return "user_preferences";
    }
    "user_profile"
}

fn has_goal_signal(entry: &MemoryEntry) -> bool {
    has_keyword(
        entry,
        &["goal", "task", "deadline", "plan", "roadmap", "release"],
    )
}

fn has_preference_signal(entry: &MemoryEntry) -> bool {
    has_keyword(
        entry,
        &[
            "prefer",
            "preference",
            "style",
            "tone",
            "language",
            "concise",
        ],
    ) || entry.memory_type == MemoryType::Procedural
}

fn has_keyword(entry: &MemoryEntry, keywords: &[&str]) -> bool {
    let haystacks = [&entry.title, &entry.value, &entry.key];
    haystacks.iter().any(|value| {
        let lower = value.to_ascii_lowercase();
        keywords.iter().any(|keyword| lower.contains(keyword))
    }) || entry.tags.iter().any(|tag| {
        keywords
            .iter()
            .any(|keyword| tag.eq_ignore_ascii_case(keyword))
    })
}

fn display_title(entry: &MemoryEntry) -> String {
    if entry.title.trim().is_empty() {
        entry.key.clone()
    } else {
        entry.title.clone()
    }
}

fn condensed_value(value: &str) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let condensed: String = chars.by_ref().take(220).collect();
    if chars.next().is_some() {
        format!("{condensed}...")
    } else {
        condensed
    }
}

fn render_item_line(item: &MemoryPackItem) -> String {
    let meta = if item.pinned {
        format!(
            "[{}/{} pinned]",
            scope_label(item.scope_type),
            memory_type_label(item.memory_type)
        )
    } else {
        format!(
            "[{}/{}]",
            scope_label(item.scope_type),
            memory_type_label(item.memory_type)
        )
    };
    format!("- {meta} {}: {}\n", item.title, item.value)
}

pub fn render_memory_pack(pack: &MemoryPack, diagnostics: &MemoryRetrievalDiagnostics) -> String {
    let mut buf = String::new();
    buf.push_str("DEEPAGENTS_MEMORY_INJECTED_V2\n");
    buf.push_str("<memory_pack>\n");
    buf.push_str(&format!("<intent>{}</intent>\n", intent_label(pack.intent)));
    for section in &pack.sections {
        buf.push_str(&format!("<{}>\n", section.name));
        for entry in &section.entries {
            buf.push_str(&render_item_line(entry));
        }
        buf.push_str(&format!("</{}>\n\n", section.name));
    }
    if !pack.warnings.is_empty() {
        buf.push_str("<memory_warnings>\n");
        for warning in &pack.warnings {
            buf.push_str("- ");
            buf.push_str(warning);
            buf.push('\n');
        }
        buf.push_str("</memory_warnings>\n\n");
    }
    buf.push_str("</memory_pack>\n\n");
    buf.push_str("<memory_guidelines>\n");
    buf.push_str(
        "The above <memory_pack> is scoped and ranked memory selected for this request.\n",
    );
    buf.push_str("Prefer pinned and higher-confidence memories, but always let explicit user corrections override stored memory.\n");
    buf.push_str("Never rely on memory from another user, thread, or workspace.\n");
    buf.push_str("If the user explicitly asks you to remember something durable, use the structured memory command path when available.\n");
    buf.push_str("</memory_guidelines>\n\n");
    buf.push_str("<memory_diagnostics>\n");
    buf.push_str(&format!(
        "mode=scoped; intent={}; selected={}; truncated={}; injected_chars={}; sections={}\n",
        intent_label(diagnostics.intent),
        diagnostics.selected.len(),
        diagnostics.truncated,
        diagnostics.injected_chars,
        diagnostics.rendered_sections.join(",")
    ));
    buf.push_str("</memory_diagnostics>\n");
    buf
}

fn intent_label(intent: MemoryIntent) -> &'static str {
    match intent {
        MemoryIntent::Personal => "personal",
        MemoryIntent::Workspace => "workspace",
        MemoryIntent::Mixed => "mixed",
        MemoryIntent::MemorizeCommand => "memorize_command",
        MemoryIntent::FactualRecall => "factual_recall",
        MemoryIntent::PlanningTask => "planning_task",
        MemoryIntent::PreferenceSensitive => "preference_sensitive",
        MemoryIntent::General => "general",
    }
}

fn scope_label(scope: MemoryScopeType) -> &'static str {
    match scope {
        MemoryScopeType::Thread => "thread",
        MemoryScopeType::User => "user",
        MemoryScopeType::Workspace => "workspace",
        MemoryScopeType::System => "system",
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Profile => "profile",
        MemoryType::Episodic => "episodic",
        MemoryType::Semantic => "semantic",
        MemoryType::Procedural => "procedural",
        MemoryType::Pinned => "pinned",
    }
}
