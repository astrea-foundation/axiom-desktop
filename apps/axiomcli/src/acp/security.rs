//! ACP security request handlers.

use super::*;

/// Prepares model keys even before a draft has a thread. The provider owns
/// freshness and per-model singleflight; account transitions cancel this work.
pub(super) async fn prewarm_security(
    context: &ServerContext,
    request: extension::PrewarmSecurityRequest,
    responder: Responder<VerifySecurityResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if !extension_enabled(&context.extension_state, ExtensionFeature::SecurityEvidence) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if request.model_id.is_empty() || request.model_id.len() > 512 {
        return responder.respond_with_internal_error("invalid model ID");
    }
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        context
            .account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let runner = context.runner.clone();
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _guard = guard;
        let result = tokio::select! {
            () = request_cancellation.cancelled() => {
                cancellation.cancel();
                Err(agent_client_protocol::Error::request_cancelled())
            }
            () = cancellation.cancelled() => Err(agent_client_protocol::Error::request_cancelled()),
            result = runner.prewarm_security(&request.model_id, cancellation.clone()) => {
                result.and_then(strict_extension_security_verification)
                    .map(|(status, evidence)| VerifySecurityResponse {
                        status: extension_security_status(status), evidence,
                    }).map_err(agent_error)
            }
        };
        responder.respond_with_result(result)
    })
}

pub(super) async fn verify_security(
    context: &ServerContext,
    request: VerifySecurityRequest,
    responder: Responder<VerifySecurityResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_runner = context.runner.clone();
    let ctx_sessions = context.sessions.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::SecurityEvidence) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let requested_internal_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid thread ID") })
    );
    let work = SessionWork::new(SessionWorkKind::SecurityVerification);
    let (external_id, internal_id, model) = {
        let mut registry = ctx_sessions.lock().await;
        let Some((external_id, session)) = registry
            .iter()
            .find(|(_, session)| session.internal_id == requested_internal_id)
        else {
            return responder.respond_with_internal_error("unknown ACP session");
        };
        let external_id = external_id.clone();
        let internal_id = session.internal_id.clone();
        let model = session.model.clone();
        if request.accept_outdated_tee && request.model_id.as_deref() != Some(model.as_str()) {
            return responder.respond_with_internal_error(
                "The selected model changed; review its warning before continuing.",
            );
        }
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error("session already has active work");
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
        registry
            .get_mut(&external_id)
            .expect("session remained loaded while reserving security verification")
            .security = crate::app::SecurityStatus::Verifying;
        (external_id, internal_id, model)
    };
    let request_cancellation = responder.cancellation();
    let task_connection = connection.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_external_id = external_id.clone();
    let rollback_internal_id = internal_id.clone();
    let rollback_model = model.clone();
    let rollback_work = work.clone();
    let account_guard = match ctx_account_work.register(work.cancellation.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            set_session_security_if_work(
                &ctx_sessions,
                &external_id,
                &internal_id,
                &work,
                &model,
                crate::app::SecurityStatus::Unverified,
            )
            .await;
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let spawn_result = connection.spawn(async move {
        let _account_guard = account_guard;
        let connection = task_connection;
        let task_result: agent_client_protocol::Result<_> = async {
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                Some(&external_id),
                ActivityEvent::SecurityChanged {
                    status: extension_security_status(crate::app::SecurityStatus::Verifying),
                },
            )?;
            let verify = async {
                if request.accept_outdated_tee {
                    ctx_runner
                        .accept_outdated_tee(&model, work.cancellation.clone())
                        .await
                } else {
                    ctx_runner
                        .verify_security(&model, work.cancellation.clone())
                        .await
                }
            };
            let verification = await_session_future(&request_cancellation, &work, verify).await?;
            let verification = match verification {
                Ok(verification) => verification,
                Err(crate::AxiomError::Cancelled) => {
                    work.cancellation.cancel();
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                Err(error) => return Err(agent_error(error)),
            };
            let (verification_status, evidence) =
                strict_extension_security_verification(verification).map_err(agent_error)?;
            if !set_session_security_if_work(
                &ctx_sessions,
                &external_id,
                &internal_id,
                &work,
                &model,
                verification_status,
            )
            .await
            {
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            let status = extension_security_status(verification_status);
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                Some(&external_id),
                ActivityEvent::SecurityChanged {
                    status: status.clone(),
                },
            )?;
            Ok(VerifySecurityResponse { evidence, status })
        }
        .await;
        if let Err(error) = &task_result {
            let verification_status =
                if work.cancellation.is_cancelled() || error.code == (-32001).into() {
                    crate::app::SecurityStatus::Unverified
                } else if error.code == (-32010).into() {
                    crate::app::SecurityStatus::Outdated
                } else {
                    crate::app::SecurityStatus::Failed
                };
            if set_session_security_if_work(
                &ctx_sessions,
                &external_id,
                &internal_id,
                &work,
                &model,
                verification_status,
            )
            .await
            {
                let status = if matches!(
                    verification_status,
                    crate::app::SecurityStatus::Failed | crate::app::SecurityStatus::Outdated
                ) {
                    ExtensionSecurityStatus {
                        state: extension_security_state(verification_status),
                        detail: Some(error.to_string()),
                    }
                } else {
                    extension_security_status(verification_status)
                };
                let _ = send_extension_activity(
                    &connection,
                    &ctx_extension_state,
                    Some(&external_id),
                    ActivityEvent::SecurityChanged { status },
                );
            }
        }
        release_session_work(&ctx_sessions, &internal_id, &work).await;
        responder.respond_with_result(task_result)
    });
    if let Err(error) = spawn_result {
        set_session_security_if_work(
            &rollback_sessions,
            &rollback_external_id,
            &rollback_internal_id,
            &rollback_work,
            &rollback_model,
            crate::app::SecurityStatus::Unverified,
        )
        .await;
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

pub(super) async fn compact(
    context: &ServerContext,
    request: CompactRequest,
    responder: Responder<CompactResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Compaction) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if request
        .focus
        .as_ref()
        .is_some_and(|focus| focus.len() > extension::MAX_FOCUS_BYTES)
    {
        return responder.respond_with_internal_error("compaction focus is too long");
    }
    let requested_internal_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid thread ID") })
    );
    let work = SessionWork::new(SessionWorkKind::Compaction);
    let (external_id, internal_id) = {
        let mut registry = ctx_sessions.lock().await;
        let Some((external_id, session)) = registry
            .iter()
            .find(|(_, session)| session.internal_id == requested_internal_id)
        else {
            return responder.respond_with_internal_error("unknown ACP session");
        };
        let external_id = external_id.clone();
        let internal_id = session.internal_id.clone();
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error("session already has active work");
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
        (external_id, internal_id)
    };
    let request_cancellation = responder.cancellation();
    let task_connection = connection.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_internal_id = internal_id.clone();
    let rollback_work = work.clone();
    let account_guard = match ctx_account_work.register(work.cancellation.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let spawn_result = connection.spawn(async move {
        let _account_guard = account_guard;
        let connection = task_connection;
        let task_result: agent_client_protocol::Result<_> = async {
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                Some(&external_id),
                ActivityEvent::Compaction {
                    phase: CompactionPhase::Started,
                    detail: Some("Compacting conversation context".into()),
                },
            )?;
            let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
            let compact = ctx_runner.compact(
                &internal_id,
                request.focus,
                events_tx,
                work.cancellation.clone(),
            );
            tokio::pin!(compact);
            let result = loop {
                tokio::select! {
                    result = &mut compact => break result,
                    event = events_rx.recv() => {
                        let Some(event) = event else { continue };
                        let envelope = ctx_runtime
                            .emit(internal_id.clone(), event.clone())
                            .await
                            .map_err(agent_error)?;
                        let revision = match &ctx_store {
                            Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                            None => None,
                        };
                        send_event(
                            &connection,
                            &external_id,
                            event,
                            EventDelivery {
                                cwd: None,
                                extension_state: Some(&ctx_extension_state),
                                correlation_id: Some(envelope.correlation_id.to_string()),
                                revision: revision.as_ref(),
                                client_item_id: None,
                            },
                        )?;
                    }
                    () = request_cancellation.cancelled() => {
                        work.cancellation.cancel();
                        break Err(crate::AxiomError::Cancelled);
                    }
                    () = work.cancellation.cancelled() => {
                        break Err(crate::AxiomError::Cancelled);
                    }
                }
            };
            match result {
                Ok(result) => {
                    let messages_before = result.messages_before;
                    let events = ctx_runtime
                        .dispatch(AppCommand::RecordContextCompaction {
                            session_id: internal_id.clone(),
                            summary: result.summary,
                            messages_before,
                        })
                        .await
                        .map_err(agent_error)?;
                    let revision = match &ctx_store {
                        Some(store) => Some(store.append_all(&events).map_err(agent_error)?),
                        None => None,
                    };
                    for event in events {
                        send_event(
                            &connection,
                            &external_id,
                            event.event,
                            EventDelivery {
                                cwd: None,
                                extension_state: Some(&ctx_extension_state),
                                correlation_id: Some(event.correlation_id.to_string()),
                                revision: revision.as_ref(),
                                client_item_id: None,
                            },
                        )?;
                    }
                    Ok(CompactResponse {
                        messages_before: u64::try_from(messages_before).unwrap_or(u64::MAX),
                    })
                }
                Err(error) => {
                    send_extension_activity(
                        &connection,
                        &ctx_extension_state,
                        Some(&external_id),
                        ActivityEvent::Compaction {
                            phase: if matches!(error, crate::AxiomError::Cancelled) {
                                CompactionPhase::Cancelled
                            } else {
                                CompactionPhase::Failed
                            },
                            detail: Some(error.to_string()),
                        },
                    )?;
                    Err(agent_error(error))
                }
            }
        }
        .await;
        release_session_work(&ctx_sessions, &internal_id, &work).await;
        responder.respond_with_result(task_result)
    });
    if let Err(error) = spawn_result {
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}
