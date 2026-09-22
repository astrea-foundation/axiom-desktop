//! ACP prompts request handlers.

use super::*;

fn publish_title(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    store: &SessionStore,
    internal_id: &SessionId,
) -> agent_client_protocol::Result<()> {
    let summary = store.thread_summary(internal_id).map_err(agent_error)?;
    if let Some(title) = summary.title {
        let revision = crate::session::ThreadRevision {
            revision: summary.revision,
            last_timeline_sequence: summary.last_timeline_sequence,
            last_message_at: summary.last_message_at,
            last_user_message_at: summary.last_user_message_at,
            timeline_item_id: None,
        };
        send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::SessionInfoUpdate(
                protocol::SessionInfoUpdate::new().title(title),
            ),
            Some(&revision),
            None,
            None,
        )?;
    }
    Ok(())
}

pub(super) async fn prompt(
    context: &ServerContext,
    request: protocol::PromptRequest,
    responder: Responder<protocol::PromptResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_transition = context.account_transition.clone();
    let ctx_account_work = context.account_work.clone();
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_persistence_failpoint = context.persistence_failpoint.clone();
    let ctx_profile_settings_lock = context.profile_settings_lock.clone();
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let ctx_supports_elicitation = context.supports_elicitation.clone();
    let prompt_metadata = respond_or_return!(
        responder,
        prompt_metadata(request.meta.as_ref()).map_err(agent_error)
    );
    let web_enabled = prompt_web_enabled(ctx_frontend, prompt_metadata.as_ref());
    let agent_revision = prompt_metadata
        .as_ref()
        .and_then(|metadata| metadata.agent_revision)
        .unwrap_or(0);
    let message_revision = prompt_metadata
        .as_ref()
        .and_then(|metadata| metadata.revision.clone());
    let mut attachments = prompt_metadata
        .as_ref()
        .map(|metadata| metadata.attachments.clone())
        .unwrap_or_default();
    if !attachments.is_empty()
        && !extension_enabled(&ctx_extension_state, ExtensionFeature::Attachments)
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let client_item_id = prompt_metadata.map(|metadata| metadata.client_item_id);
    let prompt = match prompt_input(&request.prompt) {
        Ok((prompt, embedded)) => {
            attachments.extend(embedded);
            prompt
        }
        Err(message) => return responder.respond_with_internal_error(message),
    };
    if message_revision.is_none()
        && let Err(error) = axiom_inference::validate_prompt(&prompt, &attachments)
    {
        return responder.respond_with_internal_error(error.to_string());
    }
    let session_id = request.session_id.clone();
    // Edited user text is data, including a leading slash.
    let command = match if message_revision.is_some() || !attachments.is_empty() {
        None
    } else {
        slash::parse(&prompt)
    } {
        Some(Ok(command)) => Some(command),
        Some(Err(error)) => {
            send_text(connection, &session_id, format!("Command error: {error}"))?;
            return responder.respond(protocol::PromptResponse::new(protocol::StopReason::EndTurn));
        }
        None => None,
    };
    let deterministic_test_runner = cfg!(debug_assertions)
        && std::env::var("AXIOMCLI_TEST_RUNNER").is_ok_and(|value| !value.trim().is_empty());
    if command.is_none() && !ctx_auth.has_credential() && !deterministic_test_runner {
        send_text(
            connection,
            &session_id,
            "Authentication required. Run /login to connect an Axiom account.".into(),
        )?;
        return responder.respond(protocol::PromptResponse::new(protocol::StopReason::EndTurn));
    }
    let turn_id = command.is_none().then(TurnId::new);
    let mut work = SessionWork::new(
        turn_id
            .as_ref()
            .map_or(SessionWorkKind::SlashCommand, |turn_id| {
                SessionWorkKind::Prompt(turn_id.clone())
            }),
    );
    work.web_enabled = Some(web_enabled);
    let (internal_id, cwd, session_profile, session_model, session_thinking, should_generate_title) = {
        let mut registry = ctx_sessions.lock().await;
        let Some(session) = registry.get(&session_id) else {
            return responder.respond_with_internal_error("unknown ACP session");
        };
        let internal_id = session.internal_id.clone();
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error("session already has active work");
        }
        if ctx_frontend == FrontendKind::DesktopChat {
            let agent = respond_or_return!(
                responder,
                ctx_store
                    .as_ref()
                    .expect("desktop store")
                    .desktop_agent_settings(&internal_id)
                    .map_err(agent_error)
            );
            if agent.revision != agent_revision {
                return responder.respond_with_internal_error(
                    "Agent settings changed since this message was queued. Review and resend it.",
                );
            }
            if agent.enabled
                && (!session.cwd.is_dir()
                    || session.cwd.canonicalize().ok().as_ref() != Some(&session.cwd))
            {
                return responder.respond_with_internal_error("The Agent working directory is unavailable or has moved. Choose a folder in Agent settings before sending.");
            }
            work.agent_revision = agent.revision;
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
        let session = registry
            .get_mut(&session_id)
            .expect("session remained loaded while reserving work");
        let should_generate_title = turn_id.is_some() && !session.has_prompt;
        if turn_id.is_some() {
            session.has_prompt = true;
        }
        (
            internal_id,
            session.cwd.clone(),
            session.profile,
            session.model.clone(),
            session.thinking,
            should_generate_title,
        )
    };
    let request_cancellation = responder.cancellation();
    let task_connection = connection.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_session_id = session_id.clone();
    let rollback_internal_id = internal_id.clone();
    let rollback_work = work.clone();
    let rollback_has_prompt = should_generate_title;
    let account_guard = match ctx_account_work.register(work.cancellation.clone()) {
        Ok(guard) => Some(guard),
        Err(error) => {
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let spawn_result = connection.spawn(async move {
    let account_guard = account_guard;
    let connection = task_connection;
    if let Some(command) = command {
        let slash_result: agent_client_protocol::Result<_> = async {
        match command {
            SlashCommand::Balance | SlashCommand::Topup | SlashCommand::Redeem => {
                send_text(&connection, &session_id, "Open Balance in Desktop to view credit, top up or redeem a gift code. Enter codes only in the redemption form.".into())?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Theme(_) | SlashCommand::Web(_) | SlashCommand::Usage => {
                Err(agent_client_protocol::Error::invalid_params()
                    .data("Use the desktop appearance, Web, and usage controls; /theme, /web, and /usage are terminal-only commands."))
            }
            SlashCommand::Update => {
                send_text(&connection, &session_id, "Open Settings → Updates in Desktop, or run axiomcli update in your terminal.".into())?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Help => {
                send_text(&connection, &session_id, slash_help_text())?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Permissions => {
                send_text(
                    &connection,
                    &session_id,
                    "Open your ACP client's session mode selector to change Tools permissions.".into(),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Model => {
                let models = await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_runner.available_models(work.cancellation.clone()),
                )
                .await?
                .map_err(agent_error)?;
                let current = {
                    let mut registry = ctx_sessions.lock().await;
                    if !work_matches(&registry, &internal_id, &work) {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    let session = registry.get_mut(&session_id).ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "unknown ACP session",
                        )
                    })?;
                    session.models.clone_from(&models);
                    session.model.clone()
                };
                send_model_config_update(
                    &connection,
                    &session_id,
                    &current,
                    &models,
                )?;
                send_text(
                    &connection,
                    &session_id,
                    "Choose a model using the client model selector.".into(),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Resume => {
                send_text(
                    &connection,
                    &session_id,
                    "Use your ACP client's session list and session/load support to resume a transcript. The interactive /resume picker is available in the terminal UI."
                        .into(),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Delete => {
                send_text(
                    &connection,
                    &session_id,
                    "The interactive multi-select /delete picker is available in the terminal UI. ACP transcript deletion will use a native client capability when the protocol defines one."
                        .into(),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Thinking(level) => {
                let details = await_session_future(&request_cancellation, &work,
                    ctx_runner.available_model_details(work.cancellation.clone())).await?.map_err(agent_error)?;
                let model = details.iter().find(|model| model.id == session_model)
                    .ok_or_else(|| agent_client_protocol::util::internal_error("current model is not in the provider catalog"))?;
                let level = reconcile_model_settings(model, level).thinking;
                let _profile_settings_guard = await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_profile_settings_lock.lock(),
                )
                .await?;
                await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_runner.set_thinking_level(&internal_id, level),
                )
                .await?
                .map_err(agent_error)?;
                let events = match ctx_runtime
                    .dispatch(AppCommand::ChangeThinkingLevel {
                        session_id: internal_id.clone(),
                        level,
                    })
                    .await
                {
                    Ok(events) => events,
                    Err(error) => {
                        if let Err(rollback_error) = ctx_runner
                            .set_thinking_level(&internal_id, session_thinking)
                            .await
                        {
                            tracing::warn!(
                                %rollback_error,
                                %internal_id,
                                "could not roll back runner thinking after Runtime rejected slash command"
                            );
                        }
                        return Err(agent_error(error));
                    }
                };
                let current = {
                    let mut registry = ctx_sessions.lock().await;
                    if !work_matches(&registry, &internal_id, &work) {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    let session = registry.get_mut(&session_id).ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "unknown ACP session",
                        )
                    })?;
                    session.thinking = level;
                    Some((session.model.clone(), session.models.clone()))
                };
                let stored_preferences = if let Some(store) = &ctx_store {
                    let persistence = ctx_persistence_failpoint
                        .check("slash-thinking")
                        .and_then(|()| {
                            store
                                .append_all_and_set_profile_preferences(
                                    &events,
                                    &session_model,
                                    level,
                                )
                                .map(|(_, preferences)| preferences)
                        });
                    match persistence {
                        Ok(preferences) => {
                            Some(extension_preferences(preferences))
                        }
                        Err(error) => {
                            if let Err(rollback_error) = ctx_runner
                                .set_thinking_level(&internal_id, session_thinking)
                                .await
                            {
                                tracing::error!(
                                    %rollback_error,
                                    %internal_id,
                                    "could not roll back runner after slash thinking persistence failed"
                                );
                            }
                            if let Err(rollback_error) = ctx_runtime
                                .dispatch(AppCommand::ChangeThinkingLevel {
                                    session_id: internal_id.clone(),
                                    level: session_thinking,
                                })
                                .await
                            {
                                tracing::error!(
                                    %rollback_error,
                                    %internal_id,
                                    "could not roll back Runtime after slash thinking persistence failed"
                                );
                            }
                            let mut registry = ctx_sessions.lock().await;
                            if work_matches(&registry, &internal_id, &work)
                                && let Some(session) = registry.get_mut(&session_id)
                            {
                                session.thinking = session_thinking;
                            }
                            return Err(agent_error(error));
                        }
                    }
                } else {
                    None
                };
                if let Some((model, models)) = current {
                    send_config_update(
                        &connection,
                        &session_id,
                        &model,
                        &models,
                        level,
                    )?;
                    if let Some(preferences) = stored_preferences {
                        send_extension_activity(
                            &connection,
                            &ctx_extension_state,
                            None,
                            ActivityEvent::ProfilePreferencesChanged { preferences },
                        )?;
                    }
                }
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Login => {
                let login = await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_auth.start_native_login(None),
                )
                .await?
                .map_err(agent_error)?;
                send_text(
                    &connection,
                    &session_id,
                    format!(
                        "Authorize AxiomCLI in your browser. Confirm code {}.\n{}",
                        login.user_code(),
                        login.authorization_url()
                    ),
                )?;
                let completion =
                    login.complete(work.cancellation.clone());
                tokio::pin!(completion);
                let completion_result = tokio::select! {
                    biased;
                    result = &mut completion => result,
                    () = request_cancellation.cancelled() => {
                        work.cancellation.cancel();
                        // NativeLogin may already be committing
                        // the credential. Keep polling it so a
                        // successful commit is always announced.
                        completion.await
                    }
                    () = work.cancellation.cancelled() => {
                        // session/cancel only requests cancellation;
                        // NativeLogin decides whether the commit
                        // point has already been crossed.
                        completion.await
                    }
                };
                let account = completion_result.map_err(agent_error)?;
                let _account_transition = ctx_account_transition.lock().await;
                begin_account_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    Some(&work),
                    account_guard.as_ref(),
                )
                .await
                .map_err(agent_error)?;
                publish_account_switch_started(
                    &connection,
                    &ctx_extension_state,
                    &ctx_auth,
                )
                .await?;
                commit_account_store_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    ctx_store.as_ref(),
                    Some(&account.account.id),
                )
                .await
                .map_err(agent_error)?;
                // The credential is now persisted. Finish account
                // validation and publish it without observing later
                // request/session cancellation.
                let validation = validation_status_if_current(
                    &ctx_auth,
                    ctx_auth.validate_with_revision().await,
                );
                if !matches!(&validation.status, ValidationStatus::Valid(_))
                {
                    begin_account_switch(
                        &ctx_sessions,
                        &ctx_account_work,
                        Some(&work),
                        account_guard.as_ref(),
                    )
                    .await
                    .map_err(agent_error)?;
                    commit_account_store_switch(
                        &ctx_sessions,
                        &ctx_account_work,
                        ctx_store.as_ref(),
                        None,
                    )
                    .await
                    .map_err(agent_error)?;
                }
                let status = extension_account_status(validation);
                send_extension_activity(
                    &connection,
                    &ctx_extension_state,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Logout => {
                let _account_transition = ctx_account_transition.lock().await;
                begin_account_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    Some(&work),
                    account_guard.as_ref(),
                )
                .await
                .map_err(agent_error)?;
                publish_account_switch_started(
                    &connection,
                    &ctx_extension_state,
                    &ctx_auth,
                )
                .await?;
                commit_account_store_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    ctx_store.as_ref(),
                    None,
                )
                .await
                .map_err(agent_error)?;
                let logout = ctx_auth
                    .logout_with_cancellation(&work.cancellation);
                tokio::pin!(logout);
                let logout_result = tokio::select! {
                    biased;
                    result = &mut logout => result,
                    () = request_cancellation.cancelled() => {
                        work.cancellation.cancel();
                        logout.await
                    }
                    () = work.cancellation.cancelled() => logout.await,
                };
                let revision = logout_result.map_err(agent_error)?;
                send_extension_activity(
                    &connection,
                    &ctx_extension_state,
                    None,
                    ActivityEvent::AccountChanged {
                        status: extension_account_status(VersionedValidationStatus {
                            status: ValidationStatus::Missing,
                            revision,
                        }),
                    },
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Account => {
                let validation = await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_auth.validate(),
                )
                .await?;
                let text = match validation {
                    ValidationStatus::Missing => {
                        "Not signed in. Run /login to connect AxiomCLI.".to_owned()
                    }
                    ValidationStatus::Valid(account) => format!(
                        "Axiom account session is valid: {} (stored in {}).",
                        account.account.display_name.as_deref().unwrap_or(&account.account.id),
                        account.source.label()
                    ),
                    ValidationStatus::Expired => {
                        "The saved Axiom account session expired or was revoked."
                            .to_owned()
                    }
                    ValidationStatus::Unavailable(message) => message,
                };
                send_text(&connection, &session_id, text)?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::AcceptOutdatedTee => {
                let verification = await_session_future(&request_cancellation, &work,
                    ctx_runner.accept_outdated_tee(&session_model, work.cancellation.clone())).await?.map_err(agent_error)?;
                strict_extension_security_verification(verification.clone()).map_err(agent_error)?;
                send_extension_activity(&connection, &ctx_extension_state, Some(&session_id), ActivityEvent::SecurityChanged {
                    status: extension_security_status(verification.status),
                })?;
                send_text(&connection, &session_id, "Outdated TEE accepted for this provider until restart. All other verification checks remain required.".into())?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Security => {
                let verification = await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_runner.verify_security(
                        &session_model,
                        work.cancellation.clone(),
                    ),
                )
                .await?
                .map_err(agent_error)?;
                strict_extension_security_verification(verification.clone())
                    .map_err(agent_error)?;
                send_text(
                    &connection,
                    &session_id,
                    security_summary(&verification),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Refresh => {
                {
                    let mut registry = ctx_sessions.lock().await;
                    if !work_matches(&registry, &internal_id, &work) {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    let session = registry.get_mut(&session_id).ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "unknown ACP session",
                        )
                    })?;
                    if session.model != session_model {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    session.security = crate::app::SecurityStatus::Verifying;
                }
                if let Err(error) = send_extension_activity(
                    &connection,
                    &ctx_extension_state,
                    Some(&session_id),
                    ActivityEvent::SecurityChanged {
                        status: extension_security_status(
                            crate::app::SecurityStatus::Verifying,
                        ),
                    },
                ) {
                    set_session_security_if_work(
                        &ctx_sessions,
                        &session_id,
                        &internal_id,
                        &work,
                        &session_model,
                        crate::app::SecurityStatus::Unverified,
                    )
                    .await;
                    return Err(error);
                }
                let verification = match await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_runner.verify_security(
                        &session_model,
                        work.cancellation.clone(),
                    ),
                )
                .await
                {
                    Ok(Ok(verification)) => verification,
                    Err(error) => {
                        if set_session_security_if_work(
                            &ctx_sessions,
                            &session_id,
                            &internal_id,
                            &work,
                            &session_model,
                            crate::app::SecurityStatus::Unverified,
                        )
                        .await
                        {
                            send_extension_activity(
                                &connection,
                                &ctx_extension_state,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(
                                        crate::app::SecurityStatus::Unverified,
                                    ),
                                },
                            )?;
                        }
                        return Err(error);
                    }
                    Ok(Err(crate::AxiomError::Cancelled)) => {
                        work.cancellation.cancel();
                        if set_session_security_if_work(
                            &ctx_sessions,
                            &session_id,
                            &internal_id,
                            &work,
                            &session_model,
                            crate::app::SecurityStatus::Unverified,
                        )
                        .await
                        {
                            send_extension_activity(
                                &connection,
                                &ctx_extension_state,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(
                                        crate::app::SecurityStatus::Unverified,
                                    ),
                                },
                            )?;
                        }
                        return Err(
                            agent_client_protocol::Error::request_cancelled(),
                        );
                    }
                    Ok(Err(error)) => {
                        let status = if matches!(&error, crate::AxiomError::SecureProvider {
                            code: "PROVIDER_TDX_OUT_OF_DATE", ..
                        }) {
                            crate::app::SecurityStatus::Outdated
                        } else {
                            crate::app::SecurityStatus::Failed
                        };
                        if set_session_security_if_work(
                            &ctx_sessions,
                            &session_id,
                            &internal_id,
                            &work,
                            &session_model,
                            status,
                        )
                        .await
                        {
                            send_extension_activity(
                                &connection,
                                &ctx_extension_state,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: ExtensionSecurityStatus {
                                        state: extension_security_state(status),
                                        detail: Some(error.to_string()),
                                    },
                                },
                            )?;
                        }
                        return Err(agent_error(error));
                    }
                };
                let verification_status =
                    match strict_extension_security_verification(verification.clone()) {
                        Ok(result) => result,
                        Err(error) => {
                            if set_session_security_if_work(
                                &ctx_sessions,
                                &session_id,
                                &internal_id,
                                &work,
                                &session_model,
                                crate::app::SecurityStatus::Failed,
                            )
                            .await
                            {
                                send_extension_activity(
                                    &connection,
                                    &ctx_extension_state,
                                    Some(&session_id),
                                    ActivityEvent::SecurityChanged {
                                        status: ExtensionSecurityStatus {
                                            state: ExtensionSecurityState::Failed,
                                            detail: Some(error.to_string()),
                                        },
                                    },
                                )?;
                            }
                            return Err(agent_error(error));
                        }
                    }
                    .0;
                if !set_session_security_if_work(
                    &ctx_sessions,
                    &session_id,
                    &internal_id,
                    &work,
                    &session_model,
                    verification_status,
                )
                .await
                {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                send_extension_activity(
                    &connection,
                    &ctx_extension_state,
                    Some(&session_id),
                    ActivityEvent::SecurityChanged {
                        status: extension_security_status(verification_status),
                    },
                )?;
                send_text(
                    &connection,
                    &session_id,
                    format!(
                        "Attestation refreshed.\n{}",
                        security_summary(&verification)
                    ),
                )?;
                Ok(protocol::StopReason::EndTurn)
            }
            SlashCommand::Compact { focus } => {
                let task_runtime = ctx_runtime.clone();
                let task_runner = ctx_runner.clone();
                let task_connection = connection.clone();
                let task_store = ctx_store.clone();
                let task_extension = ctx_extension_state.clone();
                    let (events_tx, mut events_rx) =
                        mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                    let task_result: agent_client_protocol::Result<_> = async {
                        let compact = task_runner.compact(
                            &internal_id,
                            focus,
                            events_tx,
                            work.cancellation.clone(),
                        );
                        tokio::pin!(compact);
                        let result = loop {
                            tokio::select! {
                                result = &mut compact => break result,
                                event = events_rx.recv() => {
                                    let Some(event) = event else { continue };
                                    let envelope = task_runtime
                                        .emit(internal_id.clone(), event.clone())
                                        .await
                                        .map_err(agent_error)?;
                                    let revision = match &task_store {
                                        Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                        None => None,
                                    };
                                    send_event(
                                        &task_connection,
                                        &session_id,
                                        event,
                                        EventDelivery {
                                            cwd: None,
                                            extension_state: Some(&task_extension),
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
                        while let Ok(event) = events_rx.try_recv() {
                            let envelope = task_runtime
                                .emit(internal_id.clone(), event.clone())
                                .await
                                .map_err(agent_error)?;
                            let revision = match &task_store {
                                Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                None => None,
                            };
                            send_event(
                                &task_connection,
                                &session_id,
                                event,
                                EventDelivery {
                                    cwd: None,
                                    extension_state: Some(&task_extension),
                                    correlation_id: Some(envelope.correlation_id.to_string()),
                                    revision: revision.as_ref(),
                                    client_item_id: None,
                                },
                            )?;
                        }
                        match result {
                            Ok(result) => {
                                let messages_before = result.messages_before;
                                let events = task_runtime
                                    .dispatch(AppCommand::RecordContextCompaction {
                                        session_id: internal_id.clone(),
                                        summary: result.summary,
                                        messages_before,
                                    })
                                    .await
                                    .map_err(agent_error)?;
                                if let Some(store) = &task_store {
                                    store.append_all(&events).map_err(agent_error)?;
                                }
                                send_text(
                                    &task_connection,
                                    &session_id,
                                    format!(
                                        "Context compacted from {messages_before} conversation message(s)."
                                    ),
                                )?;
                                Ok(protocol::StopReason::EndTurn)
                            }
                            Err(crate::AxiomError::Cancelled) => {
                                Ok(protocol::StopReason::Cancelled)
                            }
                            Err(error) => {
                                send_text(
                                    &task_connection,
                                    &session_id,
                                    format!("AxiomCLI compaction error: {error}"),
                                )?;
                                Ok(protocol::StopReason::EndTurn)
                            }
                        }
                    }
                    .await;
                task_result
            }
        }
        }
        .await;
        release_session_work(&ctx_sessions, &internal_id, &work).await;
        return responder.respond_with_result(
            slash_result.map(protocol::PromptResponse::new),
        );
    }
    let turn_id = turn_id.expect("non-slash prompt reserves a turn ID");

    let revision_history = if let Some(revision) = &message_revision {
        let prepared = (|| {
            if work.cancellation.is_cancelled() || request_cancellation.is_cancelled() {
                return Err(crate::AxiomError::Cancelled);
            }
            let store = ctx_store.as_ref().ok_or_else(|| crate::AxiomError::Storage("durable thread storage is unavailable".into()))?;
            let history = store.history_before_user_message(&internal_id, revision)?;
            // Revision changes text. Preserve the original local file/image inputs.
            attachments = store.prompt_attachments(&internal_id, &revision.user_item_id)?;
            Ok(history)
        })();
        match prepared {
            Ok(history) => Some(history),
            Err(error) => {
                release_session_work(&ctx_sessions, &internal_id, &work).await;
                return responder.respond_with_error(agent_error(error));
            }
        }
    } else { None };

    let preflight: crate::Result<()> = async {
        axiom_inference::validate_prompt(&prompt, &attachments)
            .map_err(|error| crate::AxiomError::Protocol(error.to_string()))?;
        let mut file_types = attachments.iter().filter_map(|attachment| match attachment {
            axiom_inference::PromptAttachment::File { file, .. } => Some(file.mime_type.clone()), _ => None,
        }).collect::<Vec<_>>();
        if let Some(history) = revision_history.as_ref() {
            for event in history.iter().rev().take_while(|event| !matches!(event.event, AppEvent::ContextCompacted { .. })) {
                if let AppEvent::PromptAccepted { attachments, .. } = &event.event {
                    file_types.extend(attachments.iter().filter_map(|attachment| match attachment {
                        axiom_inference::PromptAttachment::File { file, .. } => Some(file.mime_type.clone()), _ => None,
                    }));
                }
            }
        }
        if !file_types.is_empty() {
            let models = ctx_runner.available_model_details(work.cancellation.child_token()).await?;
            if !models.iter().any(|model| model.id == session_model && file_types.iter().all(|mime| model.file_mime_types.contains(mime))) {
                return Err(crate::AxiomError::Provider("This model does not support these file uploads. Choose a model with the required upload support.".into()));
            }
        }
        let history_has_images = revision_history.as_ref().is_some_and(|history| {
            history.iter().rev().take_while(|event| !matches!(event.event, AppEvent::ContextCompacted { .. })).any(|event| {
                matches!(&event.event, AppEvent::PromptAccepted { attachments, .. } if attachments.iter().any(|attachment| matches!(attachment, axiom_inference::PromptAttachment::Image { .. })))
            })
        });
        if history_has_images || attachments.iter().any(|attachment| matches!(attachment, axiom_inference::PromptAttachment::Image { .. })) {
            let models = ctx_runner.available_model_details(work.cancellation.child_token()).await?;
            if !models.iter().any(|model| model.id == session_model && model.supports_images) {
                return Err(crate::AxiomError::Provider("This model does not support encrypted image input. Choose an image-capable model before sending.".into()));
            }
        }
        Ok(())
    }.await;
    if let Err(error) = preflight {
        if should_generate_title {
            let mut registry = ctx_sessions.lock().await;
            if work_matches(&registry, &internal_id, &work) && let Some(session) = registry.get_mut(&session_id) {
                session.has_prompt = false;
            }
        }
        release_session_work(&ctx_sessions, &internal_id, &work).await;
        return responder.respond_with_error(agent_error(error));
    }

    let submitted = match ctx_runtime
        .dispatch(AppCommand::SubmitPrompt {
            session_id: internal_id.clone(),
            turn_id: turn_id.clone(),
            text: prompt.clone(),
            attachments: attachments.clone(),
        })
        .await
    {
        Ok(submitted) => submitted,
        Err(error) => {
            if should_generate_title {
                let mut registry = ctx_sessions.lock().await;
                if work_matches(&registry, &internal_id, &work)
                    && let Some(session) = registry.get_mut(&session_id)
                {
                    session.has_prompt = false;
                }
            }
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let preparation: agent_client_protocol::Result<_> = async {
        let submitted_revision = if let Some(store) = &ctx_store {
            let revision = store
                .append_prompt_revision(
                    &submitted,
                    client_item_id.as_deref(),
                    message_revision.as_ref(),
                )
                .map_err(agent_error)?;
            if let Some(history) = &revision_history {
                ctx_runner.restore_session(&internal_id, history).await.map_err(agent_error)?;
            }
            let preferences = extension_preferences(
                store
                    .set_profile_preferences(&session_model, session_thinking)
                    .map_err(agent_error)?,
            );
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                None,
                ActivityEvent::ProfilePreferencesChanged { preferences },
            )?;
            Some(revision)
        } else {
            None
        };
        for envelope in &submitted {
            send_event(
                &connection,
                &session_id,
                envelope.event.clone(),
                EventDelivery {
                    cwd: Some(&cwd),
                    extension_state: Some(&ctx_extension_state),
                    correlation_id: Some(envelope.correlation_id.to_string()),
                    revision: submitted_revision.as_ref(),
                    client_item_id: client_item_id.as_deref(),
                },
            )?;
        }
        Ok(submitted_revision)
    }
    .await;
    let _submitted_revision = match preparation {
        Ok(revision) => revision,
        Err(error) => {
            fail_active_prompt(
                &ctx_sessions,
                &ctx_runtime,
                ctx_store.as_ref(),
                &internal_id,
                &work,
                &error.to_string(),
            )
            .await;
            return responder.respond_with_error(error);
        }
    };

    let title_cancellation = CancellationToken::new();
    if should_generate_title
        && let Some(title_store) = ctx_store.as_ref()
    {
        let prepared = ctx_account_work.register_store_with_cancellation(title_store, title_cancellation.clone())
            .and_then(|account_store| {
                let job = crate::session_title::TitleGeneration::prepare(&account_store.store, &internal_id, &prompt, &session_model)?;
                Ok((account_store, job))
            });
        match prepared {
            Ok((account_store, job)) => {
                if let Err(error) = account_store.publish(|| publish_title(&connection, &session_id, &account_store.store, &internal_id)) {
                    tracing::debug!(%error, "could not publish initial title");
                }
                if let Some(job) = job {
                    let title_runner = ctx_runner.clone();
                    let title_connection = connection.clone();
                    let title_session = session_id.clone();
                    let title_internal_id = internal_id.clone();
                    let title_extension = ctx_extension_state.clone();
                    if let Err(error) = connection.spawn(async move {
                        let result = job.run(title_runner.as_ref(), account_store.cancellation.clone()).await;
                        if let Err(error) = result {
                            tracing::debug!(%error, "background title kept its local fallback");
                        }
                        let publication = account_store.publish(|| {
                            if let Ok(snapshot) = account_store.store.thread_snapshot(&title_internal_id, None, 1) {
                                for usage in snapshot.request_usage.into_iter().filter(|usage| usage.purpose == axiom_inference::InvocationPurpose::Title) {
                                    send_extension_activity(&title_connection, &title_extension, Some(&title_session), ActivityEvent::RequestUsageChanged { usage })?;
                                }
                                publish_title(&title_connection, &title_session, &account_store.store, &title_internal_id)?;
                            }
                            Ok(())
                        });
                        // A background cancellation (including an account switch) must
                        // never shut down the ACP connection or its foreground prompts.
                        if let Err(error) = publication {
                            tracing::debug!(%error, "background title publication skipped");
                        }
                        Ok(())
                    }) {
                        tracing::debug!(%error, "could not start background title request");
                    }
                }
            }
            Err(error) => tracing::warn!(session_id = %internal_id, %error, "could not persist ACP session title"),
        }
    }

    let task_sessions = ctx_sessions.clone();
    let task_runtime = ctx_runtime.clone();
    let task_runner = ctx_runner.clone();
    let task_connection = connection.clone();
    let task_store = ctx_store.clone();
    let task_elicitation = ctx_supports_elicitation.clone();
    let task_extension = ctx_extension_state.clone();
        let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
        let display_cwd = cwd.clone();
        let mut terminalized = false;
        let mut task_result: agent_client_protocol::Result<_> = async {
            let run = task_runner.run(
                crate::agent::TurnContext {
                    session_id: internal_id.clone(),
                    turn_id: turn_id.clone(),
                    cwd,
                    permission_profile: session_profile,
                    web_enabled,
                    attachments,
                    steering: work.steering.clone(),
                    approval: Some(Arc::new(AcpApproval {
                        connection: task_connection.clone(),
                        session_id: session_id.clone(),
                    })),
                    questions: Some(Arc::new(AcpQuestions {
                        connection: task_connection.clone(),
                        session_id: session_id.clone(),
                        supported: task_elicitation,
                    })),
                },
                prompt,
                events_tx,
                work.cancellation.clone(),
            );
            tokio::pin!(run);

            let result = loop {
                tokio::select! {
                    result = &mut run => break result,
                    event = events_rx.recv() => {
                        let Some(event) = event else { continue };
                        let envelope = task_runtime
                            .emit(internal_id.clone(), event.clone())
                            .await
                            .map_err(agent_error)?;
                        let revision = match &task_store {
                            Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                            None => None,
                        };
                        if let AppEvent::ThinkingLevelChanged { level } = &event
                            && let Some(session) = task_sessions.lock().await.sessions.get_mut(&session_id) { session.thinking = *level; }
                        if let AppEvent::SteeringApplied { client_item_id, .. } = &event
                            && let Some(inbox) = &work.steering { inbox.acknowledge(client_item_id); }
                        send_event(
                            &task_connection,
                            &session_id,
                            event,
                            EventDelivery {
                                cwd: Some(&display_cwd),
                                extension_state: Some(&task_extension),
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
            if result.is_err() {
                work.cancellation.cancel();
                title_cancellation.cancel();
            }
            while let Ok(event) = events_rx.try_recv() {
                let envelope = task_runtime
                    .emit(internal_id.clone(), event.clone())
                    .await
                    .map_err(agent_error)?;
                let revision = match &task_store {
                    Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                    None => None,
                };
                if let AppEvent::ThinkingLevelChanged { level } = &event
                    && let Some(session) = task_sessions.lock().await.sessions.get_mut(&session_id) { session.thinking = *level; }
                if let AppEvent::SteeringApplied { client_item_id, .. } = &event
                    && let Some(inbox) = &work.steering { inbox.acknowledge(client_item_id); }
                send_event(
                    &task_connection,
                    &session_id,
                    event,
                    EventDelivery {
                        cwd: Some(&display_cwd),
                        extension_state: Some(&task_extension),
                        correlation_id: Some(envelope.correlation_id.to_string()),
                        revision: revision.as_ref(),
                        client_item_id: None,
                    },
                )?;
            }

            let (command, stop_reason, display_error) = match result {
                Ok(()) => (
                    AppCommand::FinishTurn {
                        session_id: internal_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                    protocol::StopReason::EndTurn,
                    None,
                ),
                Err(crate::AxiomError::Cancelled) => (
                    AppCommand::CancelTurn {
                        session_id: internal_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                    protocol::StopReason::Cancelled,
                    None,
                ),
                Err(error) => {
                    let message = error.to_string();
                    (
                        AppCommand::FailTurn {
                            session_id: internal_id.clone(),
                            turn_id: turn_id.clone(),
                            message: message.clone(),
                        },
                        protocol::StopReason::EndTurn,
                        Some(message),
                    )
                }
            };
            let completed = task_runtime
                .dispatch(command)
                .await
                .map_err(agent_error)?;
            terminalized = true;
            if let Some(store) = &task_store {
                store.append_all(&completed).map_err(agent_error)?;
            }
            if let Some(error) = display_error {
                send_text(
                    &task_connection,
                    &session_id,
                    format!("AxiomCLI error: {error}"),
                )?;
            }
            Ok(stop_reason)
        }
        .await;

        if matches!(task_result, Ok(protocol::StopReason::Cancelled))
            && let Some(store) = &task_store
        {
            // Stop drops the runner future promptly to close upstream.
            // Rebuild its context from the committed journal before
            // releasing this thread; the dropped future may not have
            // saved the accepted prompt or its interrupted response.
            task_result = async {
                let history = store.load(&internal_id).map_err(agent_error)?;
                task_runner.restore_session(&internal_id, &history).await.map_err(agent_error)?;
                Ok(protocol::StopReason::Cancelled)
            }.await;
        }

        if let Err(error) = &task_result
            && !terminalized
        {
            fail_active_prompt(
                &task_sessions,
                &task_runtime,
                task_store.as_ref(),
                &internal_id,
                &work,
                &error.to_string(),
            )
            .await;
        } else {
            release_session_work(&task_sessions, &internal_id, &work).await;
        }
        responder.respond_with_result(
            task_result.map(protocol::PromptResponse::new),
        )
    });
    if let Err(error) = spawn_result {
        if rollback_has_prompt {
            let mut registry = rollback_sessions.lock().await;
            if work_matches(&registry, &rollback_internal_id, &rollback_work)
                && let Some(session) = registry.get_mut(&rollback_session_id)
            {
                session.has_prompt = false;
            }
        }
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

pub(super) async fn steer_turn(
    context: &ServerContext,
    request: extension::SteerTurnRequest,
    responder: Responder<extension::SteerTurnResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Steering) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let turn = respond_or_return!(
        responder,
        TurnId::from_str(&request.expected_turn_id)
            .map_err(|_| agent_client_protocol::util::internal_error("invalid expected turn ID"))
    );
    let registry = ctx_sessions.lock().await;
    let external = protocol::SessionId::new(request.thread_id.clone());
    let Some(session) = registry.get(&external) else {
        return responder.respond_with_internal_error("unknown ACP session");
    };
    let Some(work) = registry.active_work.get(&session.internal_id) else {
        return responder.respond_with_internal_error("turn is no longer active");
    };
    if work.web_enabled != Some(request.web_enabled)
        || work.cancellation.is_cancelled()
        || work.agent_revision != request.agent_revision.unwrap_or(0)
    {
        return responder.respond_with_internal_error("steering cannot change the active turn's Web access or Agent settings, or resume cancellation");
    }
    let Some(inbox) = &work.steering else {
        return responder.respond_with_internal_error("current work does not accept steering");
    };
    let receiver = respond_or_return!(
        responder,
        inbox
            .submit(&turn, request.client_item_id.clone(), request.text)
            .map_err(agent_error)
    );
    let cancellation = work.cancellation.clone();
    drop(registry);
    connection.spawn(async move {
        tokio::select! {
            biased;
            result = receiver => match result {
                Ok(()) => responder.respond(extension::SteerTurnResponse { turn_id: request.expected_turn_id, client_item_id: request.client_item_id }),
                Err(_) => responder.respond_with_internal_error("steering was not applied; message remains queued"),
            },
            () = cancellation.cancelled() => responder.respond_with_internal_error("turn stopped before steering was confirmed; check the saved transcript before retrying"),
        }
    })?;
    Ok(())
}

pub(super) async fn cancel(
    context: &ServerContext,
    notification: protocol::CancelNotification,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_sessions = context.sessions.clone();
    let token = {
        let registry = ctx_sessions.lock().await;
        registry
            .get(&notification.session_id)
            .and_then(|session| registry.active_work.get(&session.internal_id))
            .map(|work| work.cancellation.clone())
    };
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}
