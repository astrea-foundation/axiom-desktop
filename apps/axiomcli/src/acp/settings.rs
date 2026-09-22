//! ACP settings request handlers.

use super::*;

pub(super) async fn configure_desktop_agent(
    context: &ServerContext,
    request: extension::ConfigureDesktopAgentRequest,
    responder: Responder<extension::ConfigureDesktopAgentResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    if ctx_frontend != FrontendKind::DesktopChat
        || !extension_enabled(&ctx_extension_state, ExtensionFeature::DesktopAgent)
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("account storage unavailable");
    };
    let store = respond_or_return!(responder, store.bind_active_account().map_err(agent_error));
    let internal_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID"))
    );
    let external_id = protocol::SessionId::new(request.thread_id.clone());
    let work = SessionWork::new(SessionWorkKind::Configuration);
    let account_guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(work.cancellation.clone())
            .map_err(agent_error)
    );
    {
        let mut registry = ctx_sessions.lock().await;
        if !registry.contains_key(&external_id) {
            return responder
                .respond_with_internal_error("open the thread before changing Agent settings");
        }
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error(
                "wait for the current operation before changing Agent settings",
            );
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
    }
    let sessions = ctx_sessions.clone();
    let runtime = ctx_runtime.clone();
    let rollback_sessions = sessions.clone();
    let rollback_id = internal_id.clone();
    let rollback_work = work.clone();
    let request_cancellation = responder.cancellation();
    let spawn_result = connection.spawn(async move {
        let _account_guard = account_guard;
        let result: agent_client_protocol::Result<_> = async {
            let settings = store
                .prepare_desktop_agent_settings(&internal_id, &request)
                .map_err(agent_error)?;
            let mut previous = store
                .desktop_agent_settings(&internal_id)
                .map_err(agent_error)?;
            ctx_runner
                .reset_session_permissions(&internal_id)
                .await
                .map_err(agent_error)?;
            let mut registry = sessions.lock().await;
            if work.cancellation.is_cancelled()
                || request_cancellation.is_cancelled()
                || !work_matches(&registry, &internal_id, &work)
            {
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            let session = registry
                .get_mut(&external_id)
                .ok_or_else(|| agent_client_protocol::util::internal_error("thread closed"))?;
            previous.working_directory = session.cwd.to_string_lossy().into_owned();
            let events = runtime
                .dispatch(AppCommand::ConfigureDesktopAgent {
                    session_id: internal_id.clone(),
                    settings: settings.clone(),
                })
                .await
                .map_err(agent_error)?;
            if let Err(error) = store.append_all(&events) {
                runtime
                    .dispatch(AppCommand::ConfigureDesktopAgent {
                        session_id: internal_id.clone(),
                        settings: previous,
                    })
                    .await
                    .map_err(agent_error)?;
                return Err(agent_error(error));
            }
            session.cwd = PathBuf::from(&settings.working_directory);
            session.profile = PermissionProfile::for_desktop_agent(&settings);
            Ok(extension::ConfigureDesktopAgentResponse {
                thread: extension_thread_summary(
                    store.thread_summary(&internal_id).map_err(agent_error)?,
                ),
                agent: settings,
            })
        }
        .await;
        release_session_work(&sessions, &internal_id, &work).await;
        responder.respond_with_result(result)
    });
    if let Err(error) = spawn_result {
        release_session_work(&rollback_sessions, &rollback_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

pub(super) async fn set_session_mode(
    context: &ServerContext,
    request: protocol::SetSessionModeRequest,
    responder: Responder<protocol::SetSessionModeResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_frontend = context.frontend;
    let ctx_persistence_failpoint = context.persistence_failpoint.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let profile = respond_or_return!(
        responder,
        PermissionProfile::from_str(request.mode_id.0.as_ref()).map_err(agent_error)
    );
    if ctx_frontend == FrontendKind::DesktopChat {
        return responder.respond_with_error(extension_rpc_error(
            -32003,
            ExtensionErrorCode::PermissionDenied,
            "desktop permissions must be changed through the Agent controls",
            false,
        ));
    }
    let work = SessionWork::new(SessionWorkKind::PermissionMode);
    let (internal_id, previous_profile) = {
        let mut registry = ctx_sessions.lock().await;
        let Some(session) = registry.get(&request.session_id) else {
            return responder.respond_with_internal_error("unknown ACP session");
        };
        let internal_id = session.internal_id.clone();
        let previous_profile = session.profile;
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error(
                "cannot change permissions while the session has active work",
            );
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
        (internal_id, previous_profile)
    };
    let task_sessions = ctx_sessions.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_internal_id = internal_id.clone();
    let rollback_work = work.clone();
    let task_connection = connection.clone();
    let request_cancellation = responder.cancellation();
    let account_guard = match ctx_account_work.register(work.cancellation.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let spawn_result = connection.spawn(async move {
        let _account_guard = account_guard;
        let task_result: agent_client_protocol::Result<_> = async {
            let mut registry = task_sessions.lock().await;
            if request_cancellation.is_cancelled()
                || work.cancellation.is_cancelled()
                || !work_matches(&registry, &internal_id, &work)
            {
                work.cancellation.cancel();
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            // Permission changes are local and short. Holding the registry
            // lock makes this the atomic commit point: session/cancel can
            // win before dispatch, but cannot split Runtime, storage, and
            // the ACP session mirror after dispatch starts.
            let events = ctx_runtime
                .dispatch(AppCommand::ChangePermissionProfile {
                    session_id: internal_id.clone(),
                    profile,
                })
                .await
                .map_err(agent_error)?;
            let Some(session) = registry.get_mut(&request.session_id) else {
                return Err(agent_client_protocol::util::internal_error(
                    "unknown ACP session",
                ));
            };
            session.profile = profile;
            drop(registry);
            if let Some(store) = &ctx_store
                && let Err(error) = ctx_persistence_failpoint
                    .check("mode")
                    .and_then(|()| store.append_all(&events).map(|_| ()))
            {
                if let Err(rollback_error) = ctx_runtime
                    .dispatch(AppCommand::ChangePermissionProfile {
                        session_id: internal_id.clone(),
                        profile: previous_profile,
                    })
                    .await
                {
                    tracing::error!(
                        %rollback_error,
                        %internal_id,
                        "could not roll back Runtime after permission persistence failed"
                    );
                }
                let mut registry = task_sessions.lock().await;
                if work_matches(&registry, &internal_id, &work)
                    && let Some(session) = registry.get_mut(&request.session_id)
                {
                    session.profile = previous_profile;
                }
                return Err(agent_error(error));
            }
            task_connection.send_notification(protocol::SessionNotification::new(
                request.session_id.clone(),
                protocol::SessionUpdate::CurrentModeUpdate(protocol::CurrentModeUpdate::new(
                    profile.to_string(),
                )),
            ))?;
            Ok(protocol::SetSessionModeResponse::new())
        }
        .await;
        release_session_work(&task_sessions, &internal_id, &work).await;
        responder.respond_with_result(task_result)
    });
    if let Err(error) = spawn_result {
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

pub(super) async fn set_session_config_option(
    context: &ServerContext,
    request: protocol::SetSessionConfigOptionRequest,
    responder: Responder<protocol::SetSessionConfigOptionResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_persistence_failpoint = context.persistence_failpoint.clone();
    let ctx_profile_settings_lock = context.profile_settings_lock.clone();
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let Some(value) = request.value.as_value_id() else {
        return responder
            .respond_with_internal_error("session config requires a selected value ID");
    };
    let selected = value.0.to_string();
    if !matches!(
        request.config_id.0.as_ref(),
        MODEL_CONFIG_ID | THINKING_CONFIG_ID
    ) {
        return responder.respond_with_internal_error("unknown session config option");
    }
    let work = SessionWork::new(SessionWorkKind::Configuration);
    let (internal_id, previous_session) = {
        let mut registry = ctx_sessions.lock().await;
        let Some(session) = registry.get(&request.session_id) else {
            return responder.respond_with_internal_error("unknown ACP session");
        };
        let internal_id = session.internal_id.clone();
        let previous_session = session.clone();
        if registry.active_work.contains_key(&internal_id) {
            return responder.respond_with_internal_error(
                "cannot change settings while the session has active work",
            );
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
        (internal_id, previous_session)
    };
    let model = previous_session.model.clone();
    let thinking = previous_session.thinking;
    let request_cancellation = responder.cancellation();
    let task_sessions = ctx_sessions.clone();
    let rollback_sessions = ctx_sessions.clone();
    let rollback_internal_id = internal_id.clone();
    let rollback_work = work.clone();
    let task_connection = connection.clone();
    let account_guard = match ctx_account_work.register(work.cancellation.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            release_session_work(&ctx_sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let spawn_result = connection.spawn(async move {
        let _account_guard = account_guard;
        let task_result: agent_client_protocol::Result<_> = async {
    let _profile_settings_guard = await_session_future(
        &request_cancellation,
        &work,
        ctx_profile_settings_lock.lock(),
    )
    .await?;
    let details = await_session_future(
        &request_cancellation,
        &work,
        ctx_runner.available_model_details(work.cancellation.clone()),
    )
    .await?
    .map_err(agent_error)?;
    let mut models = details
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    let previous_model = details.iter().find(|candidate| candidate.id == model);
    let previous_settings = previous_model.map_or_else(
        || crate::agent::ModelSettings {
            model: model.clone(),
            thinking,
            supports_reasoning: false,
        },
        |model| reconcile_model_settings(model, thinking),
    );
    let previous_history = if previous_model.is_none() {
        Some(ctx_store.as_ref().ok_or_else(|| agent_client_protocol::util::internal_error("unavailable model requires saved history"))?
            .load(&internal_id).map_err(agent_error)?)
    } else { None };
    let (settings, supported_thinking, events) = match request.config_id.0.as_ref() {
        MODEL_CONFIG_ID => {
            let Some(selected_model) =
                details.iter().find(|candidate| candidate.id == selected)
            else {
                return Err(agent_client_protocol::util::internal_error(
                    "model is not in the provider catalog",
                ));
            };
            let model_changed = selected_model.id != model;
            let settings = reconcile_model_settings(selected_model, thinking);
            await_session_future(
                &request_cancellation,
                &work,
                ctx_runner
                    .set_model_settings(&internal_id, &settings),
            )
            .await?
            .map_err(agent_error)?;
            // A successful runner setter is the configuration
            // commit point. Finish Runtime, durable storage, and the
            // ACP mirror even if cancellation arrives afterward.
            let events = match ctx_runtime
                .dispatch(AppCommand::ChangeModelSettings {
                        session_id: internal_id.clone(),
                        model: settings.model.clone(),
                        thinking: settings.thinking,
                        reset_security: model_changed,
                    })
                .await
            {
                Ok(events) => events,
                Err(error) => {
                    if let Err(rollback_error) = restore_model_selection(ctx_runner.as_ref(), &internal_id, &previous_settings, previous_history.as_deref()).await
                    {
                        tracing::warn!(
                            %rollback_error,
                            %internal_id,
                            "could not roll back runner model after Runtime rejected ACP config"
                        );
                    }
                    return Err(agent_error(error));
                }
            };
            (
                settings,
                supported_thinking_levels(selected_model),
                events,
            )
        }
        THINKING_CONFIG_ID => {
            let level = crate::app::ThinkingLevel::from_str(&selected)
                .map_err(agent_error)?;
            let Some(selected_model) =
                details.iter().find(|candidate| candidate.id == model)
            else {
                return Err(agent_client_protocol::util::internal_error(
                    "current model is not in the provider catalog",
                ));
            };
            let settings = reconcile_model_settings(selected_model, level);
            let level = settings.thinking;
            await_session_future(
                &request_cancellation,
                &work,
                ctx_runner
                    .set_thinking_level(&internal_id, level),
            )
            .await?
            .map_err(agent_error)?;
            // Keep the same post-setter commit rule as model changes
            // so runner and durable session settings stay aligned.
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
                        .set_thinking_level(&internal_id, thinking)
                        .await
                    {
                        tracing::warn!(
                            %rollback_error,
                            %internal_id,
                            "could not roll back runner thinking after Runtime rejected ACP config"
                        );
                    }
                    return Err(agent_error(error));
                }
            };
            (
                settings,
                supported_thinking_levels(selected_model),
                events,
            )
        }
        _ => unreachable!("configuration ID was validated above"),
    };
    let reset_security = events.iter().any(|event| {
        matches!(
            &event.event,
            AppEvent::SecurityStatusChanged {
                state: crate::app::SecurityStatus::Unverified
            }
        )
    });
    let next_model = settings.model;
    let next_thinking = settings.thinking;
    {
        let mut registry = task_sessions.lock().await;
        if !work_matches(&registry, &internal_id, &work) {
            return Err(agent_client_protocol::Error::request_cancelled());
        }
        let Some(session) = registry.get_mut(&request.session_id) else {
            return Err(agent_client_protocol::util::internal_error(
                "unknown ACP session",
            ));
        };
        apply_session_model_settings(
            session,
            &next_model,
            next_thinking,
            &models,
            reset_security,
        );
    }
    let updated_preferences = if let Some(store) = &ctx_store {
        let persistence = ctx_persistence_failpoint
            .check("config")
            .and_then(|()| {
                store.append_all_and_set_profile_preferences(
                    &events,
                    &next_model,
                    next_thinking,
                )
            });
        match persistence {
            Ok((_, stored)) => Some(extension_preferences(stored)),
            Err(error) => {
                if let Err(rollback_error) = restore_model_selection(ctx_runner.as_ref(), &internal_id, &previous_settings, previous_history.as_deref()).await
                {
                    tracing::error!(
                        %rollback_error,
                        %internal_id,
                        "could not roll back runner after ACP config persistence failed"
                    );
                }
                if let Err(rollback_error) = ctx_runtime
                    .dispatch(AppCommand::ChangeModelSettings {
                        session_id: internal_id.clone(),
                        model: previous_session.model.clone(),
                        thinking: previous_session.thinking,
                        reset_security: false,
                    })
                    .await
                {
                    tracing::error!(
                        %rollback_error,
                        %internal_id,
                        "could not roll back Runtime after ACP config persistence failed"
                    );
                }
                if reset_security
                    && let Err(rollback_error) = ctx_runtime
                        .emit(
                            internal_id.clone(),
                            AppEvent::SecurityStatusChanged {
                                state: previous_session.security,
                            },
                        )
                        .await
                {
                    tracing::error!(
                        %rollback_error,
                        %internal_id,
                        "could not restore Runtime security after ACP config persistence failed"
                    );
                }
                let mut registry = task_sessions.lock().await;
                if work_matches(&registry, &internal_id, &work)
                    && let Some(session) = registry.get_mut(&request.session_id)
                {
                    *session = previous_session.clone();
                }
                return Err(agent_error(error));
            }
        }
    } else {
        None
    };
    if let Some(preferences) = updated_preferences {
        send_extension_activity(
            &task_connection,
            &ctx_extension_state,
            None,
            ActivityEvent::ProfilePreferencesChanged { preferences },
        )?;
    }
    if reset_security {
        send_extension_activity(
            &task_connection,
            &ctx_extension_state,
            Some(&request.session_id),
            ActivityEvent::SecurityChanged {
                status: extension_security_status(
                    crate::app::SecurityStatus::Unverified,
                ),
            },
        )?;
    }
    Ok(protocol::SetSessionConfigOptionResponse::new(
        session_config_options(
            &next_model,
            &models,
            next_thinking,
            Some(&supported_thinking),
        ),
    ))
    }
    .await;
        release_session_work(&task_sessions, &internal_id, &work).await;
        responder.respond_with_result(task_result)
    });
    if let Err(error) = spawn_result {
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}

async fn restore_model_selection(
    runner: &dyn crate::agent::TurnRunner,
    session: &crate::app::SessionId,
    previous: &crate::agent::ModelSettings,
    history: Option<&[crate::app::EventEnvelope]>,
) -> crate::Result<()> {
    if let Some(history) = history {
        runner.restore_session(session, history).await
    } else {
        runner.set_model_settings(session, previous).await
    }
}
