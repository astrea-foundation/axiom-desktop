//! ACP threads request handlers.

use super::*;

pub(super) fn list_threads(
    context: &ServerContext,
    request: &ListThreadsRequest,
    responder: Responder<ListThreadsResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ThreadCatalog) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("durable session storage is unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    if request
        .query
        .as_ref()
        .is_some_and(|query| query.len() > extension::MAX_QUERY_BYTES)
    {
        return responder.respond_with_internal_error("session query is too long");
    }
    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());
    let limit = usize::try_from(request.limit.unwrap_or(200).clamp(1, 200)).unwrap_or(200);
    let page = respond_or_return!(
        responder,
        account_store
            .read(|store| store.thread_catalog(
                query,
                request.include_archived,
                request.cursor.as_deref(),
                limit,
            ))
            .map_err(agent_error)
    );
    account_store.publish(|| {
        responder.respond(ListThreadsResponse {
            threads: page
                .threads
                .into_iter()
                .map(extension_thread_summary)
                .collect(),
            next_cursor: page.next_cursor,
        })
    })
}

pub(super) fn rename_thread(
    context: &ServerContext,
    request: &extension::RenameThreadRequest,
    responder: Responder<extension::RenameThreadResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ThreadCatalog) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("durable thread storage is unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let thread_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid thread ID") })
    );
    let thread = respond_or_return!(
        responder,
        account_store
            .mutate(|store| {
                store.rename(&thread_id, &request.title)?;
                store.thread_summary(&thread_id)
            })
            .map_err(agent_error)
    );
    account_store.publish(|| {
        responder.respond(extension::RenameThreadResponse {
            thread: extension_thread_summary(thread),
        })
    })
}

pub(super) async fn get_thread_timeline(
    context: &ServerContext,
    request: GetThreadTimelineRequest,
    responder: Responder<GetThreadTimelineResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_runner = context.runner.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Timeline) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("durable thread storage is unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let thread_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid thread ID") })
    );
    let limit = usize::try_from(
        request
            .limit
            .unwrap_or(250)
            .clamp(1, extension::MAX_TIMELINE_PAGE_SIZE),
    )
    .unwrap_or(250);
    let mut snapshot = respond_or_return!(
        responder,
        account_store
            .read(|store| {
                store.thread_page(
                    &thread_id,
                    request.after_sequence,
                    request.after_request_id.as_deref(),
                    limit,
                )
            })
            .map_err(agent_error)
    );
    let ids = if request.after_sequence.is_none() && request.after_request_id.is_none() {
        respond_or_return!(
            responder,
            account_store
                .read(|store| store.pending_accounting_ids(&thread_id))
                .map_err(agent_error)
        )
    } else {
        Vec::new()
    };
    if !ids.is_empty()
        && let Ok(Ok(records)) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ctx_runner.recover_accounting(&ids, CancellationToken::new()),
        )
        .await
    {
        let _ = account_store.read(|store| store.reconcile_request_usage(&thread_id, &records));
        snapshot = respond_or_return!(
            responder,
            account_store
                .read(|store| store.thread_page(
                    &thread_id,
                    request.after_sequence,
                    request.after_request_id.as_deref(),
                    limit
                ))
                .map_err(agent_error)
        );
    }
    account_store.publish(|| {
        let mut response = GetThreadTimelineResponse {
            desktop_agent: if ctx_frontend == FrontendKind::DesktopChat {
                Some(
                    account_store
                        .read(|store| store.desktop_agent_settings(&thread_id))
                        .map_err(agent_error)?,
                )
            } else {
                None
            },
            active_turn_id: snapshot
                .turns
                .iter()
                .find(|turn| turn.status == crate::session::TurnStatus::Running)
                .map(|turn| turn.id.clone()),
            thread: extension_thread_summary(snapshot.thread),
            context_usage: snapshot.context_usage,
            request_usage: snapshot.request_usage,
            items: snapshot
                .items
                .into_iter()
                .map(extension_timeline_item)
                .collect(),
            next_cursor: snapshot.next_cursor,
            next_request_usage_cursor: snapshot.next_request_usage_cursor,
        };
        // Wire-only tool titles can add bytes. Reserve space for the RPC envelope
        // and shorten the page if needed, preserving the last returned cursor.
        while serde_json::to_vec(&response)
            .map_err(|error| agent_client_protocol::util::internal_error(error.to_string()))?
            .len()
            > 63 * 1024 * 1024
        {
            if response.items.len() <= 1 {
                return responder
                    .respond_with_internal_error("timeline item exceeds local frame limit");
            }
            response.items.pop();
            response.next_cursor = response.items.last().map(|item| item.sequence);
        }
        responder.respond(response)
    })
}

pub(super) async fn delete_preview(
    context: &ServerContext,
    request: DeletePreviewRequest,
    responder: Responder<DeletePreviewResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ThreadCatalog) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("durable session storage is unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let ids = respond_or_return!(
        responder,
        validate_session_ids(&request.thread_ids).map_err(agent_error)
    );
    {
        let registry = ctx_sessions.lock().await;
        if ids.iter().any(|id| registry.active_work.contains_key(id)) {
            return responder.respond_with_internal_error(
                "sessions with active work must be cancelled before deletion",
            );
        }
    }
    let summaries = respond_or_return!(
        responder,
        account_store
            .read(|store| {
                ids.iter()
                    .map(|id| store.thread_summary(id).map(extension_thread_summary))
                    .collect::<crate::Result<Vec<_>>>()
            })
            .map_err(agent_error)
    );
    let token = uuid::Uuid::new_v4().to_string();
    let expires_at_unix_seconds = now_unix_seconds().saturating_add(60);
    let mut pending = ctx_extension_state.pending_deletes.lock().await;
    account_store.publish(|| {
        pending.retain(|_, grant| grant.expires_at_unix_seconds > now_unix_seconds());
        pending.insert(
            token.clone(),
            PendingDelete {
                session_ids: ids,
                expires_at_unix_seconds,
            },
        );
        responder.respond(DeletePreviewResponse {
            confirmation_token: token,
            expires_at_unix_seconds,
            threads: summaries,
        })
    })
}

pub(super) async fn delete_confirm(
    context: &ServerContext,
    request: DeleteConfirmRequest,
    responder: Responder<DeleteConfirmResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ThreadCatalog) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = ctx_store.clone() else {
        return responder.respond_with_internal_error("durable session storage is unavailable");
    };
    let requested = match validate_session_ids(&request.thread_ids) {
        Ok(requested) => requested,
        Err(error) => return responder.respond_with_internal_error(error),
    };
    let Some(grant) = ctx_extension_state
        .pending_deletes
        .lock()
        .await
        .remove(&request.confirmation_token)
    else {
        return responder
            .respond_with_internal_error("delete confirmation is missing or already used");
    };
    if grant.expires_at_unix_seconds <= now_unix_seconds() {
        return responder.respond_with_internal_error("delete confirmation expired");
    }
    if requested != grant.session_ids {
        return responder
            .respond_with_internal_error("delete selection does not match the preview");
    }
    let delete_work = SessionWork::new(SessionWorkKind::Deletion);
    {
        let mut registry = ctx_sessions.lock().await;
        if requested
            .iter()
            .any(|id| registry.active_work.contains_key(id))
        {
            return responder.respond_with_internal_error(
                "sessions with active work must be cancelled before deletion",
            );
        }
        for id in &requested {
            registry.active_work.insert(id.clone(), delete_work.clone());
        }
    }
    let request_cancellation = responder.cancellation();
    let task_sessions = ctx_sessions.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_requested = requested.clone();
    let rollback_work = delete_work.clone();
    let account_store = match ctx_account_work
        .register_store_with_cancellation(&store, delete_work.cancellation.clone())
    {
        Ok(account_store) => account_store,
        Err(error) => {
            release_session_works(&ctx_sessions, &requested, &delete_work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let account_store = Arc::new(account_store);
    let spawn_result = connection.spawn(async move {
        let task_result: agent_client_protocol::Result<_> = async {
            if request_cancellation.is_cancelled() || delete_work.cancellation.is_cancelled() {
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            let deletion_account_store = account_store.clone();
            let deletion_ids = requested.clone();
            let deleted = tokio::task::spawn_blocking(move || {
                deletion_account_store.mutate(|store| store.delete_sessions(&deletion_ids))
            })
            .await
            .map_err(|error| {
                agent_client_protocol::util::internal_error(format!(
                    "session deletion worker failed: {error}"
                ))
            })?
            .map_err(agent_error)?;
            task_sessions
                .lock()
                .await
                .retain(|_, session| !requested.contains(&session.internal_id));
            Ok(DeleteConfirmResponse {
                deleted: u32::try_from(deleted).unwrap_or(u32::MAX),
            })
        }
        .await;
        let response_result = match task_result {
            Ok(response) => account_store.publish(|| responder.respond(response)),
            Err(error) => responder.respond_with_error(error),
        };
        release_session_works(&task_sessions, &requested, &delete_work).await;
        response_result
    });
    if let Err(error) = spawn_result {
        release_session_works(&rollback_sessions, &rollback_requested, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

/// Full inputs are fetched on demand, never broadcast with every state update.
pub(super) fn get_attachments(
    context: &ServerContext,
    request: &extension::GetAttachmentsRequest,
    responder: Responder<extension::GetAttachmentsResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if !extension_enabled(&context.extension_state, ExtensionFeature::Attachments) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &context.store else {
        return responder.respond_with_internal_error("durable thread storage is unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        context
            .account_work
            .register_store(store)
            .map_err(agent_error)
    );
    let thread_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID"))
    );
    let attachments = respond_or_return!(
        responder,
        account_store
            .read(|store| store.prompt_attachments(&thread_id, &request.user_item_id))
            .map_err(agent_error)
    );
    account_store.publish(|| responder.respond(extension::GetAttachmentsResponse { attachments }))
}
