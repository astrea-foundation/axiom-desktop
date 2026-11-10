//! Public durable records and their serialized vocabulary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AxiomError, Result,
    app::{EventEnvelope, SessionId, ThinkingLevel},
};

use super::DEFAULT_EXPORT_ITEMS;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub(super) const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = AxiomError;

            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(AxiomError::Storage(format!(
                        "invalid {} value `{value}` in local state",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

string_enum!(ThreadLifecycle {
    Ready => "ready",
    Running => "running",
    WaitingForApproval => "waiting_for_approval",
    WaitingForAnswer => "waiting_for_answer",
    Compacting => "compacting",
    Closed => "closed",
});

string_enum!(TurnStatus {
    Running => "running",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
    Interrupted => "interrupted",
});

string_enum!(TimelineItemKind {
    UserMessage => "user_message",
    AssistantMessage => "assistant_message",
    Reasoning => "reasoning",
    ToolCall => "tool_call",
    Plan => "plan",
    Notice => "notice",
});

string_enum!(TimelineItemStatus {
    Pending => "pending",
    InProgress => "in_progress",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
    Interrupted => "interrupted",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRevision {
    pub revision: u64,
    pub last_timeline_sequence: u64,
    pub last_message_at: Option<String>,
    #[serde(default)]
    pub last_user_message_at: Option<String>,
    /// Exact durable message segment changed by this append, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_item_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub origin: String,
    pub profile: String,
    pub selected_model: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub lifecycle: ThreadLifecycle,
    pub archived: bool,
    pub revision: u64,
    pub last_timeline_sequence: u64,
    pub created_at: String,
    pub updated_at: String,
    pub last_message_at: Option<String>,
    #[serde(default)]
    pub last_user_message_at: Option<String>,
}

/// Compact thread projection used by the terminal session picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub origin: String,
    pub profile: String,
    pub archived: bool,
    pub updated_at: String,
}

impl From<ThreadSummary> for SessionSummary {
    fn from(thread: ThreadSummary) -> Self {
        Self {
            id: thread.id,
            title: thread.title,
            cwd: thread.cwd,
            origin: thread.origin,
            profile: thread.profile,
            archived: thread.archived,
            updated_at: thread.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: String,
    pub thread_id: String,
    pub status: TurnStatus,
    pub user_item_id: Option<String>,
    pub assistant_item_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineItem {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub kind: TimelineItemKind,
    pub status: TimelineItemStatus,
    pub client_item_id: Option<String>,
    pub external_id: Option<String>,
    pub content: String,
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadSnapshot {
    pub thread: ThreadSummary,
    pub context_usage: Option<axiom_acp_extension::ContextUsage>,
    pub request_usage: Vec<axiom_inference::RequestUsage>,
    pub turns: Vec<TurnRecord>,
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<u64>,
    pub next_request_usage_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCatalogPage {
    pub threads: Vec<ThreadSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePreferences {
    pub model: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub position: i64,
    pub thread_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionState {
    pub revision: u64,
    pub collections: Vec<Collection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionChange {
    pub collection_id: Option<String>,
    pub state: CollectionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatus {
    pub session_id: SessionId,
    pub interrupted_turns: usize,
    pub interrupted_tools: Vec<InterruptedTool>,
    pub changed_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedTool {
    pub call_id: String,
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOutcome {
    pub events: Vec<EventEnvelope>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub include_prompts: bool,
    pub include_tool_output: bool,
    pub max_items: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_prompts: true,
            include_tool_output: true,
            max_items: DEFAULT_EXPORT_ITEMS,
        }
    }
}
