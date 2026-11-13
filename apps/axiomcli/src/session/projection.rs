//! Projection of durable records into runtime events.

use std::{collections::HashMap, str::FromStr as _};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::{
    AxiomError, Result,
    app::{AppEvent, CorrelationId, EventEnvelope, Origin, PermissionProfile, SessionId, TurnId},
};

use super::codec::{origin_from_name, parse_time};
use super::records::{
    ThreadSummary, TimelineItem, TimelineItemKind, TimelineItemStatus, TurnRecord, TurnStatus,
};

pub(super) fn project_runtime_events(
    id: &SessionId,
    thread: &ThreadSummary,
    turns: &[TurnRecord],
    items: &[TimelineItem],
    attachments: &HashMap<String, Vec<axiom_inference::PromptAttachment>>,
) -> Result<Vec<EventEnvelope>> {
    let origin = origin_from_name(&thread.origin)?;
    let profile = PermissionProfile::from_str(&thread.profile).map_err(|_| {
        AxiomError::Storage(format!(
            "invalid permission profile `{}` in local state",
            thread.profile
        ))
    })?;
    let mut projection = RuntimeProjection::new(id.clone(), origin);
    projection.push(
        parse_time(&thread.created_at),
        AppEvent::SessionCreated {
            cwd: thread.cwd.clone(),
            origin,
            profile,
        },
    );
    if let Some(model) = &thread.selected_model {
        projection.push(
            parse_time(&thread.created_at),
            AppEvent::ModelChanged {
                model: model.clone(),
            },
        );
    }
    projection.push(
        parse_time(&thread.created_at),
        AppEvent::ThinkingLevelChanged {
            level: thread.thinking_level,
        },
    );

    let turns = turns
        .iter()
        .map(|turn| (turn.id.as_str(), turn))
        .collect::<HashMap<_, _>>();
    let mut active_turn: Option<&str> = None;
    for item in items {
        if item.kind == TimelineItemKind::UserMessage {
            if let Some(previous) = active_turn.take()
                && Some(previous) != item.turn_id.as_deref()
            {
                project_turn_terminal(&mut projection, turns.get(previous).copied());
            }
            let turn_id = item.turn_id.as_deref().ok_or_else(|| {
                AxiomError::Storage(format!("user timeline item {} has no turn", item.id))
            })?;
            let parsed_turn = TurnId::from_str(turn_id).map_err(|error| {
                AxiomError::Storage(format!("invalid stored turn ID `{turn_id}`: {error}"))
            })?;
            if item.metadata.get("steering").and_then(Value::as_bool) == Some(true) {
                projection.push(
                    parse_time(&item.created_at),
                    AppEvent::SteeringApplied {
                        turn_id: parsed_turn,
                        client_item_id: item
                            .client_item_id
                            .clone()
                            .unwrap_or_else(|| item.id.clone()),
                        text: item.content.clone(),
                    },
                );
                active_turn = Some(turn_id);
                continue;
            }
            projection.push(
                parse_time(&item.created_at),
                AppEvent::PromptAccepted {
                    attachments: attachments.get(&item.id).cloned().unwrap_or_default(),
                    turn_id: parsed_turn.clone(),
                    text: item.content.clone(),
                },
            );
            projection.push(
                parse_time(&item.created_at),
                AppEvent::TurnStarted {
                    turn_id: parsed_turn,
                },
            );
            active_turn = Some(turn_id);
            continue;
        }
        let Some(turn_id) = item
            .turn_id
            .as_deref()
            .map(|value| {
                TurnId::from_str(value).map_err(|error| {
                    AxiomError::Storage(format!("invalid stored turn ID `{value}`: {error}"))
                })
            })
            .transpose()?
        else {
            project_thread_item(&mut projection, item)?;
            continue;
        };
        match item.kind {
            TimelineItemKind::AssistantMessage if !item.content.is_empty() => projection.push(
                parse_time(&item.updated_at),
                AppEvent::TextDelta {
                    turn_id,
                    text: item.content.clone(),
                },
            ),
            TimelineItemKind::Reasoning if !item.content.is_empty() => projection.push(
                parse_time(&item.updated_at),
                AppEvent::ReasoningDelta {
                    turn_id,
                    text: item.content.clone(),
                },
            ),
            TimelineItemKind::ToolCall => project_tool(&mut projection, item, turn_id),
            TimelineItemKind::Plan => project_plan(&mut projection, item)?,
            TimelineItemKind::Notice => project_notice(&mut projection, item),
            TimelineItemKind::UserMessage
            | TimelineItemKind::AssistantMessage
            | TimelineItemKind::Reasoning => {}
        }
    }
    if let Some(turn_id) = active_turn {
        project_turn_terminal(&mut projection, turns.get(turn_id).copied());
    }
    Ok(projection.events)
}

pub(super) struct RuntimeProjection {
    id: SessionId,
    origin: Origin,
    sequence: u64,
    events: Vec<EventEnvelope>,
}

impl RuntimeProjection {
    pub(super) fn new(id: SessionId, origin: Origin) -> Self {
        Self {
            id,
            origin,
            sequence: 0,
            events: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, occurred_at: DateTime<Utc>, event: AppEvent) {
        self.sequence = self.sequence.saturating_add(1);
        self.events.push(EventEnvelope {
            schema_version: 1,
            sequence: self.sequence,
            occurred_at,
            correlation_id: CorrelationId::new(),
            origin: self.origin,
            session_id: self.id.clone(),
            event,
        });
    }
}

pub(super) fn project_turn_terminal(projection: &mut RuntimeProjection, turn: Option<&TurnRecord>) {
    let Some(turn) = turn else { return };
    let Ok(turn_id) = TurnId::from_str(&turn.id) else {
        return;
    };
    let occurred_at = turn
        .completed_at
        .as_deref()
        .map_or_else(|| parse_time(&turn.started_at), parse_time);
    if turn.input_tokens > 0 || turn.output_tokens > 0 {
        projection.push(
            occurred_at,
            AppEvent::UsageUpdated {
                input_tokens: turn.input_tokens,
                output_tokens: turn.output_tokens,
            },
        );
    }
    let event = match turn.status {
        TurnStatus::Completed => AppEvent::TurnCompleted { turn_id },
        TurnStatus::Cancelled => AppEvent::TurnCancelled { turn_id },
        TurnStatus::Failed => AppEvent::ErrorRaised {
            turn_id: Some(turn_id),
            message: turn.error.clone().unwrap_or_else(|| "turn failed".into()),
        },
        TurnStatus::Interrupted | TurnStatus::Running => AppEvent::ErrorRaised {
            turn_id: Some(turn_id),
            message: turn.error.clone().unwrap_or_else(|| {
                "The previous process ended during this turn; unfinished work was not replayed"
                    .into()
            }),
        },
    };
    projection.push(occurred_at, event);
}

pub(super) fn project_tool(
    projection: &mut RuntimeProjection,
    item: &TimelineItem,
    turn_id: TurnId,
) {
    let call_id = item.external_id.clone().unwrap_or_else(|| item.id.clone());
    let name = item
        .metadata
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let arguments = item
        .metadata
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    projection.push(
        parse_time(&item.created_at),
        AppEvent::ToolProposed {
            turn_id,
            call_id: call_id.clone(),
            name: name.clone(),
            arguments,
        },
    );
    if item.status != TimelineItemStatus::Pending {
        projection.push(
            parse_time(&item.created_at),
            AppEvent::ToolStarted {
                call_id: call_id.clone(),
                name,
            },
        );
    }
    if !item.content.is_empty() {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::ToolOutput {
                call_id: call_id.clone(),
                content: item.content.clone(),
                truncated: item
                    .metadata
                    .get("output_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        );
    }
    if let Some(diff) = item.metadata.get("diff").and_then(Value::as_str) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::DiffAvailable {
                call_id: call_id.clone(),
                diff: diff.into(),
                truncated: item
                    .metadata
                    .get("diff_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                files: serde_json::from_value(
                    item.metadata
                        .get("files")
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                )
                .unwrap_or_default(),
            },
        );
    }
    if matches!(
        item.status,
        TimelineItemStatus::Completed | TimelineItemStatus::Failed
    ) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::ToolCompleted {
                call_id,
                success: item.status == TimelineItemStatus::Completed,
            },
        );
    }
}

pub(super) fn project_plan(projection: &mut RuntimeProjection, item: &TimelineItem) -> Result<()> {
    if item.metadata.get("code").and_then(Value::as_str) == Some("task_list") {
        if let Some(items) = item.metadata.get("items") {
            projection.push(
                parse_time(&item.updated_at),
                AppEvent::TaskListUpdated {
                    items: serde_json::from_value(items.clone()).map_err(|error| {
                        AxiomError::Storage(format!("invalid stored task list: {error}"))
                    })?,
                },
            );
        }
        return Ok(());
    }
    let Some(plan_id) = item.external_id.clone() else {
        return Ok(());
    };
    let revision = item
        .metadata
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    projection.push(
        parse_time(&item.created_at),
        AppEvent::PlanProposed {
            plan_id: plan_id.clone(),
            revision,
            markdown: item.content.clone(),
        },
    );
    if let Some(decision) = item.metadata.get("decision").and_then(Value::as_str) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::PlanReviewed {
                plan_id,
                revision,
                decision: decision.into(),
            },
        );
    }
    Ok(())
}

pub(super) fn project_thread_item(
    projection: &mut RuntimeProjection,
    item: &TimelineItem,
) -> Result<()> {
    match item.kind {
        TimelineItemKind::Plan => project_plan(projection, item),
        TimelineItemKind::Notice => {
            project_notice(projection, item);
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn project_notice(projection: &mut RuntimeProjection, item: &TimelineItem) {
    let code = item
        .metadata
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| {
            item.metadata
                .get("details")
                .and_then(|details| details.get("code"))
                .and_then(Value::as_str)
        })
        .unwrap_or("notice");
    let details = item.metadata.get("details").unwrap_or(&item.metadata);
    let event = match code {
        "progress" => Some(AppEvent::ProgressUpdated {
            message: item.content.clone(),
            completed: item.metadata.get("completed").and_then(Value::as_u64),
            total: item.metadata.get("total").and_then(Value::as_u64),
        }),
        "workspace_changed" => Some(AppEvent::WorkspaceChanged {
            paths: serde_json::from_value(
                details.get("paths").cloned().unwrap_or_else(|| json!([])),
            )
            .unwrap_or_default(),
        }),
        "context_compacted" => Some(AppEvent::ContextCompacted {
            summary: item.content.clone(),
            messages_before: details
                .get("messages_before")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0),
        }),
        "warning" => Some(AppEvent::WarningRaised {
            message: item.content.clone(),
        }),
        "error"
            if !details
                .get("terminal")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Some(AppEvent::ErrorRaised {
                turn_id: None,
                message: item.content.clone(),
            })
        }
        _ => None,
    };
    if let Some(event) = event {
        projection.push(parse_time(&item.updated_at), event);
    }
}
