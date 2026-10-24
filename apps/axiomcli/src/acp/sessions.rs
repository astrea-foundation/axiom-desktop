//! ACP sessions request handlers.

use super::*;

pub(super) fn initialize(
    context: &ServerContext,
    request: &protocol::InitializeRequest,
    responder: Responder<protocol::InitializeResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_extension_state = context.extension_state.clone();
    let ctx_supports_elicitation = context.supports_elicitation.clone();
    let ctx_supports_load = context.supports_load;
    ctx_supports_elicitation.store(
        request
            .client_capabilities
            .elicitation
            .as_ref()
            .is_some_and(|value| value.form.is_some()),
        Ordering::Release,
    );
    ctx_extension_state.negotiate(negotiated_extension_features(
        request.client_capabilities.meta.as_ref(),
    ));
    let capabilities = protocol::AgentCapabilities::new()
        .load_session(ctx_supports_load)
        .prompt_capabilities(
            protocol::PromptCapabilities::new()
                .image(true)
                .embedded_context(false),
        )
        .meta(extension_meta(ExtensionCapabilities::agent(
            ctx_extension_state.runtime_instance_id.clone(),
        )));
    responder.respond(
        protocol::InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(
                protocol::Implementation::new("axiomcli", env!("CARGO_PKG_VERSION"))
                    .title("AxiomCLI"),
            ),
    )
}

pub(super) fn desktop_bootstrap(
    context: &ServerContext,
    _request: DesktopBootstrapRequest,
    responder: Responder<DesktopBootstrapResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_default_model = context.default_model.clone();
    let ctx_default_thinking = context.default_thinking;
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::DesktopChat) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if ctx_frontend != FrontendKind::DesktopChat {
        return responder.respond_with_internal_error(
            "AxiomCLI was not launched with the desktop-chat frontend",
        );
    }
    let cwd = ctx_store
        .as_ref()
        .and_then(|store| store.active_desktop_cwd().ok());
    let durable_preferences = if let Some(store) = &ctx_store
        && store.is_active()
    {
        Some(respond_or_return!(
            responder,
            store.profile_preferences().map_err(agent_error)
        ))
    } else {
        None
    };
    let preferences = match durable_preferences {
        Some(preferences) => extension::ProfilePreferences {
            model: Some(
                preferences
                    .model
                    .unwrap_or_else(|| ctx_default_model.clone()),
            ),
            thinking_level: preferences.thinking_level.to_string(),
            updated_at: preferences.updated_at,
        },
        None => extension::ProfilePreferences {
            model: Some(ctx_default_model.clone()),
            thinking_level: ctx_default_thinking.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    };
    responder.respond(DesktopBootstrapResponse {
        frontend: ctx_frontend.to_string(),
        chat_cwd: cwd.map(|path| path.display().to_string()),
        permission_profile: PermissionProfile::Web.to_string(),
        new_thread_settings: preferences,
    })
}

pub(super) fn new_session(
    context: &ServerContext,
    request: protocol::NewSessionRequest,
    responder: Responder<protocol::NewSessionResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_config = context.config.clone();
    let ctx_default_model = context.default_model.clone();
    let ctx_default_thinking = context.default_thinking;
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let account_cancellation = CancellationToken::new();
    let account_guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(account_cancellation.clone())
            .map_err(agent_error)
    );
    let task_connection = connection.clone();
    connection.spawn(async move {
        let _account_guard = account_guard;
        let connection = task_connection;
        if !request.cwd.is_absolute() {
            return responder.respond_with_internal_error("ACP session cwd must be absolute");
        }
        let mut cwd = respond_or_return!(
            responder,
            request.cwd.canonicalize().map_err(|error| {
                agent_client_protocol::util::internal_error(format!(
                    "cannot open ACP workspace: {error}"
                ))
            })
        );
        let profile = if ctx_frontend == FrontendKind::DesktopChat {
            let expected = respond_or_return!(
                responder,
                ctx_store
                    .as_ref()
                    .ok_or_else(|| agent_client_protocol::util::internal_error(
                        "authentication required; account storage is unavailable"
                    ))
                    .and_then(|store| store.active_desktop_cwd().map_err(agent_error))
            );
            if cwd != expected {
                return responder.respond_with_internal_error(
                    "desktop chat sessions must use the application chat workspace",
                );
            }
            PermissionProfile::Web
        } else {
            respond_or_return!(
                responder,
                ctx_config.for_workspace(&cwd).map_err(agent_error)
            )
            .permission_profile
        };
        let internal_id = SessionId::new();
        if ctx_frontend == FrontendKind::DesktopChat {
            cwd = respond_or_return!(
                responder,
                ctx_store
                    .as_ref()
                    .expect("desktop store checked")
                    .active_desktop_thread_cwd(&internal_id)
                    .map_err(agent_error)
            );
        }
        let preferences = respond_or_return!(
            responder,
            ctx_store
                .as_ref()
                .map(SessionStore::profile_preferences)
                .transpose()
                .map_err(agent_error)
        );
        let preferred_model = preferences
            .as_ref()
            .and_then(|preferences| preferences.model.as_deref());
        let preferred_thinking = preferences
            .as_ref()
            .map_or(ctx_default_thinking, |preferences| {
                preferences.thinking_level
            });
        let request_cancellation = responder.cancellation();
        let discovery_cancellation = account_cancellation.clone();
        let discovery = ctx_runner.available_model_details(discovery_cancellation.clone());
        tokio::pin!(discovery);
        let model_details = tokio::select! {
            result = &mut discovery => respond_or_return!(responder, result.map_err(agent_error)),
            () = request_cancellation.cancelled() => {
                discovery_cancellation.cancel();
                return responder.respond_with_error(
                    agent_client_protocol::Error::request_cancelled(),
                );
            }
            () = account_cancellation.cancelled() => {
                return responder.respond_with_error(
                    agent_client_protocol::Error::request_cancelled(),
                );
            }
        };
        let settings = respond_or_return!(
            responder,
            reconcile_new_session_settings(
                model_details.clone(),
                preferred_model,
                &ctx_default_model,
                preferred_thinking,
            )
            .map_err(agent_error)
        );
        let correct_preferences = ctx_store.is_some()
            && preferences.as_ref().is_none_or(|preferences| {
                preferences.model.as_deref() != Some(settings.model.as_str())
                    || preferences.thinking_level != settings.thinking
            });
        let session_model = settings.model.clone();
        let session_thinking = settings.thinking;
        let models = model_details
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();
        let supported_thinking = respond_or_return!(
            responder,
            model_details
                .iter()
                .find(|model| model.id == session_model)
                .map(supported_thinking_levels)
                .ok_or_else(|| {
                    agent_client_protocol::util::internal_error(
                        "selected model disappeared from the provider catalog",
                    )
                })
        );
        let created = respond_or_return!(
            responder,
            ctx_runtime
                .dispatch(AppCommand::CreateSession {
                    session_id: internal_id.clone(),
                    cwd: cwd.clone(),
                    origin: Origin::Acp,
                    profile,
                })
                .await
                .map_err(agent_error)
        );
        if let Some(store) = &ctx_store {
            respond_or_return!(responder, store.append_all(&created).map_err(agent_error));
        }
        respond_or_return!(
            responder,
            ctx_runner
                .restore_session(&internal_id, &created)
                .await
                .map_err(agent_error)
        );
        respond_or_return!(
            responder,
            ctx_runner
                .set_model_settings(&internal_id, &settings)
                .await
                .map_err(agent_error)
        );
        let selected_settings = respond_or_return!(
            responder,
            ctx_runtime
                .dispatch(AppCommand::ChangeModelSettings {
                    session_id: internal_id.clone(),
                    model: session_model.clone(),
                    thinking: session_thinking,
                    reset_security: false,
                })
                .await
                .map_err(agent_error)
        );
        let corrected_preferences = if let Some(store) = &ctx_store {
            if correct_preferences {
                let (_, preferences) = respond_or_return!(
                    responder,
                    store
                        .append_all_and_set_profile_preferences(
                            &selected_settings,
                            &session_model,
                            session_thinking,
                        )
                        .map_err(agent_error)
                );
                Some(extension_preferences(preferences))
            } else {
                respond_or_return!(
                    responder,
                    store.append_all(&selected_settings).map_err(agent_error)
                );
                None
            }
        } else {
            None
        };
        let external_id = protocol::SessionId::new(internal_id.to_string());
        ctx_sessions.lock().await.insert(
            external_id.clone(),
            AcpSession {
                internal_id: internal_id.clone(),
                cwd,
                profile,
                model: session_model.clone(),
                thinking: session_thinking,
                models: models.clone(),
                has_prompt: false,
                security: crate::app::SecurityStatus::Unverified,
            },
        );
        responder.respond(
            protocol::NewSessionResponse::new(external_id.clone())
                .modes(permission_modes(profile, ctx_frontend))
                .config_options(session_config_options(
                    &session_model,
                    &models,
                    session_thinking,
                    Some(&supported_thinking),
                )),
        )?;
        if let Some(preferences) = corrected_preferences {
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                None,
                ActivityEvent::ProfilePreferencesChanged { preferences },
            )?;
        }
        send_available_commands(&connection, &external_id)
    })?;
    Ok(())
}

pub(super) async fn load_session(
    context: &ServerContext,
    request: protocol::LoadSessionRequest,
    responder: Responder<protocol::LoadSessionResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_default_model = context.default_model.clone();
    let ctx_default_thinking = context.default_thinking;
    let ctx_extension_state = context.extension_state.clone();
    let ctx_frontend = context.frontend;
    let ctx_runner = context.runner.clone();
    let ctx_runtime = context.runtime.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let Some(store) = ctx_store else {
        return responder.respond_with_internal_error("durable session loading is unavailable");
    };
    if !request.cwd.is_absolute() {
        return responder.respond_with_internal_error("ACP session cwd must be absolute");
    }
    let external_id = request.session_id.clone();
    let internal_id = respond_or_return!(
        responder,
        SessionId::from_str(request.session_id.0.as_ref())
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid session ID") })
    );
    let work = SessionWork::new(SessionWorkKind::Loading);
    {
        let mut registry = ctx_sessions.lock().await;
        if registry
            .values()
            .any(|session| session.internal_id == internal_id)
        {
            return responder.respond_with_internal_error("ACP session is already loaded");
        }
        if registry.active_work.contains_key(&internal_id) {
            return responder
                .respond_with_internal_error("ACP session is already loading or being deleted");
        }
        registry
            .active_work
            .insert(internal_id.clone(), work.clone());
    }
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
            let durable_store = store.clone();
            let durable_internal_id = internal_id.clone();
            let durable_load = tokio::task::spawn_blocking(move || {
                let summary = durable_store.summary(&durable_internal_id)?;
                let loaded = durable_store.load_recovering(&durable_internal_id)?;
                Ok::<_, crate::AxiomError>((summary, loaded))
            });
            let (summary, loaded) =
                await_session_future(&request_cancellation, &work, durable_load)
                    .await?
                    .map_err(|error| {
                        agent_client_protocol::util::internal_error(format!(
                            "durable session load worker failed: {error}"
                        ))
                    })?
                    .map_err(agent_error)?;
            // Desktop must be able to reopen local history even when its
            // project folder moved, so the user can select a replacement.
            // Local Agent execution revalidates the saved identity below.
            let requested_cwd = if ctx_frontend == FrontendKind::DesktopChat {
                request.cwd.clone()
            } else {
                request.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "cannot open ACP workspace: {error}"
                    ))
                })?
            };
            let saved_cwd = if ctx_frontend == FrontendKind::DesktopChat {
                summary.cwd.clone()
            } else {
                summary.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "saved ACP workspace is unavailable: {error}"
                    ))
                })?
            };
            if requested_cwd != saved_cwd {
                return Err(agent_client_protocol::util::internal_error(
                    "ACP load cwd does not match the session workspace identity",
                ));
            }
            if ctx_frontend == FrontendKind::DesktopChat {
                let agent = store
                    .desktop_agent_settings(&internal_id)
                    .map_err(agent_error)?;
                if saved_cwd != std::path::Path::new(&agent.working_directory) {
                    return Err(agent_client_protocol::util::internal_error(
                        "desktop chat session has an invalid workspace identity",
                    ));
                }
            }
            if let Some(warning) = loaded.warnings.first() {
                return Err(agent_client_protocol::util::internal_error(warning.clone()));
            }
            let restored_model = model_for_resume(&loaded.events, &ctx_default_model);
            let restored_thinking = thinking_for_resume(&loaded.events, ctx_default_thinking);
            let model_details = await_session_future(
                &request_cancellation,
                &work,
                ctx_runner.available_model_details(work.cancellation.clone()),
            )
            .await?
            .map_err(agent_error)?;
            let model_available = model_details.iter().any(|model| model.id == restored_model);
            let settings = if model_available {
                crate::agent::reconcile_new_session_settings(
                    model_details.clone(),
                    Some(&restored_model),
                    &ctx_default_model,
                    restored_thinking,
                )
                .map_err(agent_error)?
            } else {
                crate::agent::ModelSettings {
                    model: restored_model.clone(),
                    thinking: restored_thinking,
                    supports_reasoning: false,
                }
            };
            let session_model = settings.model.clone();
            let session_thinking = settings.thinking;
            let models = model_details
                .iter()
                .map(|model| model.id.clone())
                .collect::<Vec<_>>();
            let supported_thinking = model_details
                .iter()
                .find(|model| model.id == session_model)
                .map(supported_thinking_levels)
                .unwrap_or_default();
            await_session_future(
                &request_cancellation,
                &work,
                ctx_runner.restore_session(&internal_id, &loaded.events),
            )
            .await?
            .map_err(agent_error)?;
            if model_available {
                await_session_future(
                    &request_cancellation,
                    &work,
                    ctx_runner.set_model_settings(&internal_id, &settings),
                )
                .await?
                .map_err(agent_error)?;
            }
            // Completing the runner settings is the load commit point.
            // Cancellation before it leaves no registered ACP session;
            // after it, finish restoring Runtime/storage/session state so
            // the runner and ACP mirror cannot diverge.
            let restored = ctx_runtime
                .restore(&loaded.events)
                .await
                .map_err(agent_error)?;
            for (index, envelope) in loaded.events.iter().enumerate() {
                send_event(
                    &connection,
                    &external_id,
                    envelope.event.clone(),
                    EventDelivery {
                        cwd: Some(&saved_cwd),
                        extension_state: None,
                        correlation_id: Some(envelope.correlation_id.to_string()),
                        revision: None,
                        client_item_id: None,
                    },
                )?;
                if (index + 1) % 128 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let resumed = ctx_runtime
                .emit(
                    internal_id.clone(),
                    AppEvent::SessionResumed {
                        cwd: saved_cwd.clone(),
                        origin: Origin::Acp,
                        profile: restored.profile,
                    },
                )
                .await
                .map_err(agent_error)?;
            store.append(&resumed).map_err(agent_error)?;
            let model_changed = restored_model != session_model;
            if model_changed || restored_thinking != session_thinking {
                let corrected = ctx_runtime
                    .dispatch(AppCommand::ChangeModelSettings {
                        session_id: internal_id.clone(),
                        model: session_model.clone(),
                        thinking: session_thinking,
                        reset_security: model_changed,
                    })
                    .await
                    .map_err(agent_error)?;
                let revision = store.append_all(&corrected).map_err(agent_error)?;
                for envelope in corrected {
                    send_event(
                        &connection,
                        &external_id,
                        envelope.event,
                        EventDelivery {
                            cwd: Some(&saved_cwd),
                            extension_state: Some(&ctx_extension_state),
                            correlation_id: Some(envelope.correlation_id.to_string()),
                            revision: Some(&revision),
                            client_item_id: None,
                        },
                    )?;
                }
            }
            let recovery = store.recovery_status(&internal_id).map_err(agent_error)?;
            if recovery.interrupted_turns > 0 || !recovery.interrupted_tools.is_empty() {
                connection.send_notification(protocol::SessionNotification::new(
            external_id.clone(),
            protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                    "[recovery] {} interrupted turn(s), {} operation(s) unknown; nothing replayed",
                    recovery.interrupted_turns,
                    recovery.interrupted_tools.len()
                ))),
            )),
        ))?;
            }
            let restored_profile = if ctx_frontend == FrontendKind::DesktopChat {
                PermissionProfile::for_desktop_agent(
                    &store
                        .desktop_agent_settings(&internal_id)
                        .map_err(agent_error)?,
                )
            } else {
                permission_for_resume(&loaded.events, restored.profile)
            };
            let response = protocol::LoadSessionResponse::new()
                .modes(permission_modes(restored_profile, ctx_frontend))
                .config_options(session_config_options(
                    &session_model,
                    &models,
                    session_thinking,
                    Some(&supported_thinking),
                ));
            {
                let mut registry = ctx_sessions.lock().await;
                if !work_matches(&registry, &internal_id, &work) {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                if registry
                    .values()
                    .any(|session| session.internal_id == internal_id)
                {
                    return Err(agent_client_protocol::util::internal_error(
                        "ACP session became loaded while restoring",
                    ));
                }
                registry.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id: internal_id.clone(),
                        cwd: saved_cwd,
                        profile: restored_profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: models.clone(),
                        has_prompt: loaded.events.iter().any(|envelope| {
                            matches!(envelope.event, AppEvent::PromptAccepted { .. })
                        }),
                        security: crate::app::SecurityStatus::Unverified,
                    },
                );
            }
            Ok(response)
        }
        .await;
        release_session_work(&ctx_sessions, &internal_id, &work).await;
        match task_result {
            Ok(response) => {
                responder.respond(response)?;
                send_available_commands(&connection, &external_id)
            }
            Err(error) => responder.respond_with_error(error),
        }
    });
    if let Err(error) = spawn_result {
        release_session_work(&rollback_sessions, &rollback_internal_id, &rollback_work).await;
        return Err(error);
    }
    Ok(())
}
