//! ACP catalog request handlers.

use super::*;

pub(super) fn list_models(
    context: &ServerContext,
    _request: ListModelsRequest,
    responder: Responder<ListModelsResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_extension_state = context.extension_state.clone();
    let ctx_runner = context.runner.clone();
    connection.spawn(async move {
        if !extension_enabled(&ctx_extension_state, ExtensionFeature::ModelCatalog) {
            return responder.respond_with_error(extension_not_negotiated());
        }
        let request_cancellation = responder.cancellation();
        let discovery_cancellation = CancellationToken::new();
        let discovery = ctx_runner.available_model_details(discovery_cancellation.clone());
        tokio::pin!(discovery);
        let models = tokio::select! {
            result = &mut discovery => respond_or_return!(responder, result.map_err(agent_error)),
            () = request_cancellation.cancelled() => {
                discovery_cancellation.cancel();
                return responder.respond_with_error(
                    agent_client_protocol::Error::request_cancelled(),
                );
            }
        }
        .into_iter()
        .map(|model| {
            if model.id.trim().is_empty()
                || model.label.trim().is_empty()
                || model.short_label.trim().is_empty()
                || model.provider_id.trim().is_empty()
                || model.provider_label.trim().is_empty()
                || model.upstream_model.trim().is_empty()
                || model.context_window_tokens == 0
                || model.max_output_tokens == 0
                || model.max_output_tokens > model.context_window_tokens
                || !matches!(
                    (
                        model.input_price_microusd_per_million_tokens,
                        model.output_price_microusd_per_million_tokens,
                    ),
                    (None, None) | (Some(1..), Some(1..))
                )
            {
                return Err(agent_client_protocol::util::internal_error(
                    "provider catalog returned incomplete model metadata",
                ));
            }
            let thinking_levels = supported_thinking_levels(&model)
                .into_iter()
                .map(|level| level.to_string())
                .collect();
            Ok(ModelInfo {
                auto_compact_threshold_tokens: ctx_runner.auto_compact_threshold_tokens(&model),
                label: model.label,
                short_label: model.short_label,
                provider_id: model.provider_id,
                provider_label: model.provider_label,
                upstream_model: model.upstream_model,
                id: model.id,
                context_window_tokens: model.context_window_tokens,
                max_output_tokens: model.max_output_tokens,
                supports_images: model.supports_images,
                file_mime_types: model.file_mime_types.clone(),
                input_price_microusd_per_million_tokens: model
                    .input_price_microusd_per_million_tokens,
                output_price_microusd_per_million_tokens: model
                    .output_price_microusd_per_million_tokens,
                thinking_levels,
            })
        })
        .collect::<agent_client_protocol::Result<Vec<_>>>();
        let models = respond_or_return!(responder, models);
        responder.respond(ListModelsResponse { models })
    })?;
    Ok(())
}

pub(super) fn get_profile_preferences(
    context: &ServerContext,
    _request: extension::GetProfilePreferencesRequest,
    responder: Responder<ProfilePreferencesResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ProfilePreferences) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder
            .respond_with_internal_error("durable profile preferences are unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let preferences = extension_preferences(respond_or_return!(
        responder,
        account_store
            .read(SessionStore::profile_preferences)
            .map_err(agent_error)
    ));
    account_store.publish(|| responder.respond(ProfilePreferencesResponse { preferences }))
}

pub(super) fn set_profile_preferences(
    context: &ServerContext,
    request: extension::SetProfilePreferencesRequest,
    responder: Responder<ProfilePreferencesResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_profile_settings_lock = context.profile_settings_lock.clone();
    let ctx_runner = context.runner.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::ProfilePreferences) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = ctx_store.clone() else {
        return responder
            .respond_with_internal_error("durable profile preferences are unavailable");
    };
    if request
        .model
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > extension::MAX_IDENTIFIER_BYTES)
        || request
            .thinking_level
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > extension::MAX_IDENTIFIER_BYTES)
    {
        return responder.respond_with_internal_error("profile preference is invalid");
    }
    let task_extension = ctx_extension_state.clone();
    let task_runner = ctx_runner.clone();
    let task_lock = ctx_profile_settings_lock.clone();
    let task_connection = connection.clone();
    let request_cancellation = responder.cancellation();
    let account_cancellation = CancellationToken::new();
    let account_store = respond_or_return!(
        responder,
        ctx_account_work
            .register_store_with_cancellation(&store, account_cancellation.clone())
            .map_err(agent_error)
    );
    connection.spawn(async move {
        let task_result: agent_client_protocol::Result<_> = async {
            if request_cancellation.is_cancelled() {
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            let lock = task_lock.lock();
            tokio::pin!(lock);
            let _guard = tokio::select! {
                guard = &mut lock => guard,
                () = request_cancellation.cancelled() => {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                () = account_cancellation.cancelled() => {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
            };
            let current = account_store
                .read(SessionStore::profile_preferences)
                .map_err(agent_error)?;
            let Some(next_model) = request.model.or(current.model) else {
                return Err(agent_client_protocol::util::internal_error(
                    "a model must be selected before saving profile preferences",
                ));
            };
            let next_thinking = request
                .thinking_level
                .as_deref()
                .map(crate::app::ThinkingLevel::from_str)
                .transpose()
                .map_err(agent_error)?
                .unwrap_or(current.thinking_level);
            let cancellation = account_cancellation;
            let models_future = task_runner.available_model_details(cancellation.clone());
            tokio::pin!(models_future);
            let models = tokio::select! {
                result = &mut models_future => result.map_err(agent_error)?,
                () = request_cancellation.cancelled() => {
                    cancellation.cancel();
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                () = cancellation.cancelled() => {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
            };
            let Some(model) = models.iter().find(|model| model.id == next_model) else {
                return Err(agent_client_protocol::util::internal_error(
                    "model is not in the provider catalog",
                ));
            };
            let next_thinking = reconcile_model_settings(model, next_thinking).thinking;
            if request_cancellation.is_cancelled() || cancellation.is_cancelled() {
                return Err(agent_client_protocol::Error::request_cancelled());
            }
            let preferences = extension_preferences(
                account_store
                    .mutate(|store| store.set_profile_preferences(&next_model, next_thinking))
                    .map_err(agent_error)?,
            );
            Ok(preferences)
        }
        .await;
        match task_result {
            Ok(preferences) => account_store.publish(|| {
                send_extension_activity(
                    &task_connection,
                    &task_extension,
                    None,
                    ActivityEvent::ProfilePreferencesChanged {
                        preferences: preferences.clone(),
                    },
                )?;
                responder.respond(ProfilePreferencesResponse { preferences })
            }),
            Err(error) => responder.respond_with_error(error),
        }
    })?;
    Ok(())
}
