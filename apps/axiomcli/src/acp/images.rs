//! Explicit user-requested replay of one conversation-owned image recipe.
use super::{
    APP_EVENT_QUEUE_CAPACITY, AppCommand, AppEvent, Client, ConnectionTo, EventDelivery,
    ExtensionFeature, FrontendKind, Responder, ServerContext, SessionWork, SessionWorkKind, TurnId,
    agent_error, await_session_future, extension, extension_enabled, extension_not_negotiated,
    mpsc, protocol, release_session_work, send_event,
};

pub(super) async fn read(
    context: &ServerContext,
    request: extension::ReadImageRequest,
    responder: Responder<extension::ReadImageResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if !extension_enabled(&context.extension_state, ExtensionFeature::ImageResources) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if request.call_id.is_empty()
        || request.call_id.len() > 512
        || request.sha256.len() != 64
        || !request
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return responder.respond_with_internal_error("invalid image identity");
    }
    let Some(store) = &context.store else {
        return responder.respond_with_internal_error("image storage is unavailable");
    };
    let account = match context.account_work.register_store(store) {
        Ok(account) => account,
        Err(error) => return responder.respond_with_error(agent_error(error)),
    };
    let Ok(thread_id) = request.thread_id.parse::<crate::app::SessionId>() else {
        return responder.respond_with_internal_error("invalid thread ID");
    };
    // Decoding and hashing an original PNG must not block other native requests.
    let result = tokio::task::spawn_blocking(move || {
        let image = account
            .read(|store| store.generated_image(&thread_id, &request.call_id, &request.sha256));
        match image {
            Ok(image) => account.publish(|| {
                responder.respond(extension::ReadImageResponse {
                    data: image.data,
                    mime_type: image.mime_type,
                    width: image.width,
                    height: image.height,
                    sha256: image.sha256,
                })
            }),
            Err(error) => responder.respond_with_error(agent_error(error)),
        }
    })
    .await;
    result.map_err(|error| agent_client_protocol::util::internal_error(error.to_string()))?
}

fn recipe(
    history: &[crate::app::EventEnvelope],
    source: &str,
) -> crate::Result<(serde_json::Value, Option<String>)> {
    let arguments = history
        .iter()
        .find_map(|event| match &event.event {
            AppEvent::ToolProposed {
                call_id,
                name,
                arguments,
                ..
            } if call_id == source
                && matches!(name.as_str(), "generate" | "generate_image" | "edit_image") =>
            {
                Some(arguments.clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            crate::AxiomError::Tool(
                "The image request is not available in this conversation.".into(),
            )
        })?;
    let image = history
        .iter()
        .find_map(|event| match &event.event {
            AppEvent::ImageGenerated { call_id, image } if call_id == source => Some(image),
            _ => None,
        })
        .filter(|image| image.usage.response_verified && image.usage.settled)
        .ok_or_else(|| {
            crate::AxiomError::Tool("Choose a successfully generated image to regenerate.".into())
        })?;
    if !history.iter().any(|event| matches!(&event.event, AppEvent::ToolCompleted { call_id, success: true } if call_id == source)) {
        return Err(crate::AxiomError::Tool("The original image request did not complete.".into()));
    }
    let mut arguments = arguments
        .as_object()
        .cloned()
        .ok_or_else(|| crate::AxiomError::Tool("The saved image request is invalid.".into()))?;
    let prompt = arguments
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty() && prompt.len() <= 8192)
        .ok_or_else(|| crate::AxiomError::Tool("The saved image prompt is unavailable.".into()))?
        .to_owned();
    if let Some(id) = arguments.remove("image_id") {
        arguments.insert("reference_image_ids".into(), serde_json::json!([id]));
    }
    let references = arguments
        .remove("reference_image_ids")
        .unwrap_or_else(|| serde_json::json!([]));
    let ids = references
        .as_array()
        .ok_or_else(|| crate::AxiomError::Tool("The saved image references are invalid.".into()))?;
    if ids
        .iter()
        .map(|id| id.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        != image
            .source_image_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return Err(crate::AxiomError::Tool(
            "The saved image reference order differs from the original result.".into(),
        ));
    }
    Ok((
        serde_json::json!({"prompt":prompt, "reference_image_ids":references, "width":image.width, "height":image.height}),
        image.chat_model_id.clone(),
    ))
}

pub(super) async fn regenerate(
    context: &ServerContext,
    request: extension::RegenerateImageRequest,
    responder: Responder<extension::RegenerateImageResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if !extension_enabled(
        &context.extension_state,
        ExtensionFeature::ImageRegeneration,
    ) || context.frontend != FrontendKind::DesktopChat
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if request.call_id.is_empty() || request.call_id.len() > 512 {
        return responder.respond_with_internal_error("invalid image call ID");
    }
    if !(context.auth.has_credential()
        || cfg!(debug_assertions) && std::env::var("AXIOMCLI_TEST_RUNNER").is_ok())
    {
        return responder.respond_with_internal_error("Sign in before regenerating an image.");
    }
    let Some(store) = &context.store else {
        return responder.respond_with_internal_error("Image history is unavailable.");
    };
    let external_id = protocol::SessionId::new(request.thread_id);
    let turn_id = TurnId::new();
    let work = SessionWork::new(SessionWorkKind::ImageGeneration(turn_id.clone()));
    let (internal_id, cwd, profile) = {
        let mut registry = context.sessions.lock().await;
        if registry.account_switching {
            return responder.respond_with_internal_error("The account is changing.");
        }
        let Some(session) = registry.get(&external_id) else {
            return responder.respond_with_internal_error("Load the image's conversation first.");
        };
        let id = session.internal_id.clone();
        if registry.active_work.contains_key(&id) {
            return responder.respond_with_internal_error(
                "Wait for the current response or stop it before regenerating an image.",
            );
        }
        let values = (id.clone(), session.cwd.clone(), session.profile);
        registry.active_work.insert(id, work.clone());
        values
    };
    let account = match context
        .account_work
        .register_store_with_cancellation(store, work.cancellation.clone())
    {
        Ok(account) => account,
        Err(error) => {
            release_session_work(&context.sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let prepared = account.read(|store| {
        let history = store.load(&internal_id)?;
        let (arguments, origin_model) = recipe(&history, &request.call_id)?;
        Ok((history, arguments, origin_model))
    });
    let (history, arguments, origin_model) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            release_session_work(&context.sessions, &internal_id, &work).await;
            return responder.respond_with_error(agent_error(error));
        }
    };
    let call_id = format!("image_{}", uuid::Uuid::new_v4().simple());
    let runner = context.runner.clone();
    let runtime = context.runtime.clone();
    let sessions = context.sessions.clone();
    let extension = context.extension_state.clone();
    let connection_task = connection.clone();
    let request_cancellation = responder.cancellation();
    let rollback = (sessions.clone(), internal_id.clone(), work.clone());
    let spawn = connection.spawn(async move {
        let mut started = false;
        let result: agent_client_protocol::Result<()> = async {
            await_session_future(&request_cancellation, &work, runner.restore_session(&internal_id, &history)).await?.map_err(agent_error)?;
            account.ensure_current().map_err(agent_error)?;
            let beginning = runtime.dispatch(AppCommand::StartImageGeneration { session_id: internal_id.clone(), turn_id: turn_id.clone() }).await.map_err(agent_error)?;
            started = true;
            account.store.append_all(&beginning).map_err(agent_error)?;
            let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
            let run = runner.regenerate_image(crate::agent::TurnContext {
                session_id: internal_id.clone(), turn_id: turn_id.clone(), cwd: cwd.clone(),
                permission_profile: profile, web_enabled: false, image_tools_available: true,
                attachments: Vec::new(), steering: None, approval: None, questions: None, mcp: None,
            }, call_id.clone(), arguments, origin_model, events_tx, work.cancellation.clone());
            tokio::pin!(run);
            let operation = loop {
                tokio::select! {
                    biased;
                    result = &mut run => break result,
                    event = events_rx.recv() => {
                        let Some(event) = event else { continue };
                        let envelope = runtime.emit(internal_id.clone(), event.clone()).await.map_err(agent_error)?;
                        let revision = account.store.append(&envelope).map_err(agent_error)?;
                        account.publish(|| send_event(&connection_task, &external_id, event, EventDelivery {
                            cwd: Some(&cwd), extension_state: Some(&extension), correlation_id: Some(envelope.correlation_id.to_string()), revision: Some(&revision), client_item_id: None,
                        }))?;
                    }
                    () = request_cancellation.cancelled() => { work.cancellation.cancel(); break Err(crate::AxiomError::Cancelled); }
                    () = work.cancellation.cancelled() => break Err(crate::AxiomError::Cancelled),
                }
            };
            while let Ok(event) = events_rx.try_recv() {
                let envelope = runtime.emit(internal_id.clone(), event.clone()).await.map_err(agent_error)?;
                let revision = account.store.append(&envelope).map_err(agent_error)?;
                account.publish(|| send_event(&connection_task, &external_id, event, EventDelivery {
                    cwd: Some(&cwd), extension_state: Some(&extension), correlation_id: Some(envelope.correlation_id.to_string()), revision: Some(&revision), client_item_id: None,
                }))?;
            }
            operation.map_err(agent_error)
        }.await;
        let mut result = result;
        if started {
            let command = if result.is_ok() { AppCommand::FinishTurn { session_id: internal_id.clone(), turn_id: turn_id.clone() } }
                else if work.cancellation.is_cancelled() || request_cancellation.is_cancelled() { AppCommand::CancelTurn { session_id: internal_id.clone(), turn_id: turn_id.clone() } }
                else { AppCommand::FailTurn { session_id: internal_id.clone(), turn_id: turn_id.clone(), message: "Image regeneration could not finish.".into() } };
            match runtime.dispatch(command).await {
                Ok(events) => { if let Err(error) = account.store.append_all(&events) { result = Err(agent_error(error)); } }
                Err(error) => { result = Err(agent_error(error)); }
            }
            match account.store.load(&internal_id) {
                Ok(history) => { if let Err(error) = runner.restore_session(&internal_id, &history).await { result = Err(agent_error(error)); } }
                Err(error) => { result = Err(agent_error(error)); }
            }
        }
        release_session_work(&sessions, &internal_id, &work).await;
        responder.respond_with_result(result.map(|()| extension::RegenerateImageResponse { call_id }))
    });
    if let Err(error) = spawn {
        release_session_work(&rollback.0, &rollback.1, &rollback.2).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_tool::tests::generated_fixture;
    use crate::{
        app::{Origin, PermissionProfile, Runtime, SessionId},
        session::SessionStore,
    };
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn image_only_turns_preserve_the_prompt_and_original_image_across_store_replay() {
        let runtime = Runtime::new(64);
        let store = SessionStore::in_memory().unwrap();
        let id = SessionId::new();
        let turn = TurnId::new();
        store
            .append_all(
                &runtime
                    .dispatch(AppCommand::CreateSession {
                        session_id: id.clone(),
                        cwd: PathBuf::from("/tmp"),
                        origin: Origin::Test,
                        profile: PermissionProfile::Web,
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
        store
            .append_all(
                &runtime
                    .dispatch(AppCommand::SubmitPrompt {
                        session_id: id.clone(),
                        turn_id: turn.clone(),
                        text: "Original chat prompt".into(),
                        attachments: vec![],
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
        let refs = vec![
            format!("img_{}", "aa".repeat(16)),
            format!("img_{}", "bb".repeat(16)),
        ];
        let mut original = generated_fixture();
        original.chat_model_id = Some("chutes:qwen3.8-27b".into());
        original.source_image_ids = refs.clone();
        let arguments = json!({"prompt":"Use <image1> with the colors of <image2>","reference_image_ids":refs,"aspect_ratio":"1:1"});
        for event in [
            AppEvent::ToolProposed {
                mcp_source: None,
                turn_id: turn.clone(),
                call_id: "original-image".into(),
                name: "generate".into(),
                arguments: arguments.clone(),
            },
            AppEvent::ToolStarted {
                call_id: "original-image".into(),
                name: "generate".into(),
            },
            AppEvent::ImageGenerated {
                call_id: "original-image".into(),
                image: Box::new(original.clone()),
            },
            AppEvent::ToolOutput {
                call_id: "original-image".into(),
                content: "Original image created".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "original-image".into(),
                success: true,
            },
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "Original answer stays intact".into(),
            },
        ] {
            store
                .append(&runtime.emit(id.clone(), event).await.unwrap())
                .unwrap();
        }
        store
            .append_all(
                &runtime
                    .dispatch(AppCommand::FinishTurn {
                        session_id: id.clone(),
                        turn_id: turn,
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
        let history = store.load(&id).unwrap();
        let (replay, origin_model) = recipe(&history, "original-image").unwrap();
        assert_eq!(origin_model.as_deref(), Some("chutes:qwen3.8-27b"));
        assert_eq!(
            replay,
            json!({"prompt":arguments["prompt"],"reference_image_ids":refs,"width":1536,"height":1536})
        );
        assert!(recipe(&history, "foreign-or-missing-call").is_err());
        let mut changed = history.clone();
        for event in &mut changed {
            if let AppEvent::ToolProposed { arguments, .. } = &mut event.event {
                arguments["reference_image_ids"]
                    .as_array_mut()
                    .unwrap()
                    .reverse();
            }
        }
        assert!(
            recipe(&changed, "original-image").is_err(),
            "reference identity and order must match the authenticated original"
        );

        let next = TurnId::new();
        store
            .append_all(
                &runtime
                    .dispatch(AppCommand::StartImageGeneration {
                        session_id: id.clone(),
                        turn_id: next.clone(),
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
        let mut regenerated = original.clone();
        regenerated.request_id = "34".repeat(16);
        regenerated.usage.request_id = regenerated.request_id.clone();
        for event in [
            AppEvent::ToolProposed {
                mcp_source: None,
                turn_id: next.clone(),
                call_id: "new-image".into(),
                name: "generate".into(),
                arguments: replay,
            },
            AppEvent::ToolStarted {
                call_id: "new-image".into(),
                name: "generate".into(),
            },
            AppEvent::ImageGenerated {
                call_id: "new-image".into(),
                image: Box::new(regenerated),
            },
            AppEvent::ToolOutput {
                call_id: "new-image".into(),
                content: "New image created".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "new-image".into(),
                success: true,
            },
        ] {
            store
                .append(&runtime.emit(id.clone(), event).await.unwrap())
                .unwrap();
        }
        store
            .append_all(
                &runtime
                    .dispatch(AppCommand::FinishTurn {
                        session_id: id.clone(),
                        turn_id: next,
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
        let history = store.load(&id).unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|event| matches!(event.event, AppEvent::PromptAccepted { .. }))
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|event| matches!(event.event, AppEvent::ImageGenerated { .. }))
                .count(),
            2
        );
        assert_eq!(
            history
                .iter()
                .filter(|event| matches!(event.event, AppEvent::TurnCompleted { .. }))
                .count(),
            2
        );
        assert!(history.iter().any(|event| matches!(&event.event,AppEvent::TextDelta{text,..} if text=="Original answer stays intact")));
        let restored = Runtime::new(64).restore(&history).await.unwrap();
        assert!(restored.active_turn.is_none());
        assert_eq!(
            recipe(&history, "original-image").unwrap().0["width"],
            original.width
        );
        assert_eq!(
            recipe(&history, "new-image").unwrap().0["reference_image_ids"],
            json!(refs)
        );
    }
}
