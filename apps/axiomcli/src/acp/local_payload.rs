//! Account-bound draft/outbox persistence; no keys cross ACP.
use super::{
    AccountStoreWork, Client, ConnectionTo, ExtensionFeature, FrontendKind, Responder,
    ServerContext, agent_error, extension, extension_enabled, extension_not_negotiated,
};

fn execute(
    account: &AccountStoreWork,
    request: &extension::LocalPayloadRequest,
) -> crate::Result<extension::LocalPayloadResponse> {
    if request.account_id.is_empty()
        || request.account_id.len() > 512
        || request.id.is_empty()
        || request.id.len() > 2048
        || request.id.chars().any(char::is_control)
    {
        return Err(crate::AxiomError::InvalidTransition(
            "invalid local payload identity".into(),
        ));
    }
    account.ensure_current()?;
    if account.store.active_account_id().as_deref() != Some(request.account_id.as_str()) {
        return Err(crate::AxiomError::InvalidTransition(
            "local payload account changed".into(),
        ));
    }
    let mut omitted_media = None;
    let value = match &request.action {
        extension::LocalPayloadAction::Get => {
            account.read(|store| store.local_payload(&request.id))?
        }
        extension::LocalPayloadAction::Put { value } => {
            // Only the typed persistence envelopes are accepted. Attachment
            // validation and binary externalization belong to SessionStore.
            if value.get("version").and_then(serde_json::Value::as_u64) != Some(1)
                || !matches!(
                    value.get("kind").and_then(serde_json::Value::as_str),
                    Some("prompt" | "queue")
                )
            {
                return Err(crate::AxiomError::InvalidTransition(
                    "invalid local payload envelope".into(),
                ));
            }
            account.mutate(|store| store.set_local_payload(&request.id, value))?;
            None
        }
        extension::LocalPayloadAction::Remove => {
            account.mutate(|store| store.remove_local_payload(&request.id))?;
            None
        }
        extension::LocalPayloadAction::ImportLegacy { value, preview } => {
            let (normalized, omitted) =
                account.mutate(|store| store.import_local_payload(&request.id, value, *preview))?;
            omitted_media = Some(omitted);
            Some(normalized)
        }
    };
    Ok(extension::LocalPayloadResponse {
        value,
        omitted_media,
    })
}

pub(super) async fn handle(
    context: &ServerContext,
    request: extension::LocalPayloadRequest,
    responder: Responder<extension::LocalPayloadResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if context.frontend != FrontendKind::DesktopChat
        || !extension_enabled(&context.extension_state, ExtensionFeature::LocalPayload)
        || matches!(
            request.action,
            extension::LocalPayloadAction::ImportLegacy { .. }
        ) && !extension_enabled(
            &context.extension_state,
            ExtensionFeature::LegacyPayloadImport,
        )
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &context.store else {
        return responder.respond_with_internal_error("encrypted local storage is unavailable");
    };
    let account = match context.account_work.register_store(store) {
        Ok(account) => account,
        Err(error) => return responder.respond_with_error(agent_error(error)),
    };
    tokio::task::spawn_blocking(move || match execute(&account, &request) {
        Ok(response) => account.publish(|| responder.respond(response)),
        Err(error) => responder.respond_with_error(agent_error(error)),
    })
    .await
    .map_err(|error| agent_client_protocol::util::internal_error(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AccountWorkTracker;
    use std::sync::Arc;

    #[test]
    fn legacy_import_requires_local_payload_version_two() {
        let state = super::super::ExtensionState::new();
        let mut features = extension::FeatureVersions::all();
        features.local_payload = 1;
        state.negotiate(Some(features));
        assert!(state.feature_enabled(ExtensionFeature::LocalPayload));
        assert!(!state.feature_enabled(ExtensionFeature::LegacyPayloadImport));
        features.local_payload = 2;
        state.negotiate(Some(features));
        assert!(state.feature_enabled(ExtensionFeature::LegacyPayloadImport));
        state.negotiate(None);
        assert!(!state.feature_enabled(ExtensionFeature::LegacyPayloadImport));
    }

    fn request(
        owner: &str,
        action: extension::LocalPayloadAction,
    ) -> extension::LocalPayloadRequest {
        extension::LocalPayloadRequest {
            account_id: owner.into(),
            id: "draft".into(),
            action,
        }
    }

    #[test]
    fn encrypted_payload_requests_require_the_requested_owner_and_bound_generation() {
        let root = tempfile::tempdir().unwrap();
        let paths = crate::paths::AxiomPaths::from_roots(
            root.path().join("config"),
            root.path().join("data"),
        );
        paths.prepare().unwrap();
        let store = crate::session::SessionStore::account_routed(paths, FrontendKind::DesktopChat);
        store.activate_fixture_account("alice").unwrap();
        let tracker = Arc::new(AccountWorkTracker::default());
        let alice = tracker.register_store(&store).unwrap();
        let value = serde_json::json!({"version":1,"kind":"prompt","payload":{"text":"Alice private draft","attachments":[]}});
        execute(
            &alice,
            &request(
                "alice",
                extension::LocalPayloadAction::Put {
                    value: value.clone(),
                },
            ),
        )
        .unwrap();
        for action in [
            extension::LocalPayloadAction::Get,
            extension::LocalPayloadAction::Remove,
            extension::LocalPayloadAction::Put {
                value: value.clone(),
            },
            extension::LocalPayloadAction::ImportLegacy {
                value: value.clone(),
                preview: false,
            },
        ] {
            assert!(execute(&alice, &request("bob", action)).is_err());
        }
        assert_eq!(
            execute(
                &alice,
                &request("alice", extension::LocalPayloadAction::Get)
            )
            .unwrap()
            .value,
            Some(value.clone())
        );
        store.activate_fixture_account("bob").unwrap();
        assert!(
            execute(
                &alice,
                &request(
                    "alice",
                    extension::LocalPayloadAction::ImportLegacy {
                        value: value.clone(),
                        preview: false
                    }
                )
            )
            .is_err()
        );
        assert!(
            execute(
                &alice,
                &request("alice", extension::LocalPayloadAction::Remove)
            )
            .is_err()
        );
        let bob = tracker.register_store(&store).unwrap();
        assert!(
            execute(&bob, &request("bob", extension::LocalPayloadAction::Get))
                .unwrap()
                .value
                .is_none()
        );
        store.activate_fixture_account("alice").unwrap();
        assert!(
            execute(
                &alice,
                &request("alice", extension::LocalPayloadAction::Get)
            )
            .is_err(),
            "away-and-back invalidates old store work"
        );
        let current = tracker.register_store(&store).unwrap();
        assert_eq!(
            execute(
                &current,
                &request("alice", extension::LocalPayloadAction::Get)
            )
            .unwrap()
            .value,
            Some(value)
        );
        tracker.start_switch(None).unwrap();
        assert!(
            execute(
                &current,
                &request("alice", extension::LocalPayloadAction::Remove)
            )
            .is_err()
        );
    }
}
