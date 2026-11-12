//! ACP collections request handlers.

use super::*;

pub(super) fn list_collections(
    context: &ServerContext,
    _request: extension::ListCollectionsRequest,
    responder: Responder<CollectionStateResponse>,
    _connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(respond_or_return!(
        responder,
        account_store
            .read(SessionStore::list_collections)
            .map_err(agent_error)
    ));
    account_store.publish(|| responder.respond(CollectionStateResponse { state }))
}

pub(super) fn create_collection(
    context: &ServerContext,
    request: &extension::CreateCollectionRequest,
    responder: Responder<CollectionStateResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    if request.name.len() > extension::MAX_COLLECTION_NAME_BYTES {
        return responder.respond_with_internal_error("collection name is too long");
    }
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(
        respond_or_return!(
            responder,
            account_store
                .mutate(|store| store.create_collection(&request.name))
                .map_err(agent_error)
        )
        .state,
    );
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(CollectionStateResponse { state })
    })
}

pub(super) fn rename_collection(
    context: &ServerContext,
    request: &extension::RenameCollectionRequest,
    responder: Responder<CollectionStateResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    if request.name.len() > extension::MAX_COLLECTION_NAME_BYTES {
        return responder.respond_with_internal_error("collection name is too long");
    }
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(
        respond_or_return!(
            responder,
            account_store
                .mutate(|store| { store.rename_collection(&request.collection_id, &request.name) })
                .map_err(agent_error)
        )
        .state,
    );
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(CollectionStateResponse { state })
    })
}

pub(super) fn set_collection_collapsed(
    context: &ServerContext,
    request: &extension::SetCollectionCollapsedRequest,
    responder: Responder<CollectionStateResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(
        respond_or_return!(
            responder,
            account_store
                .mutate(|store| {
                    store.set_collection_collapsed(&request.collection_id, request.collapsed)
                })
                .map_err(agent_error)
        )
        .state,
    );
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(CollectionStateResponse { state })
    })
}

pub(super) fn move_collection(
    context: &ServerContext,
    request: &extension::MoveCollectionRequest,
    responder: Responder<CollectionStateResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(
        respond_or_return!(
            responder,
            account_store
                .mutate(|store| { store.move_collection(&request.collection_id, request.position) })
                .map_err(agent_error)
        )
        .state,
    );
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(CollectionStateResponse { state })
    })
}

pub(super) fn delete_collection(
    context: &ServerContext,
    request: &extension::DeleteCollectionRequest,
    responder: Responder<CollectionStateResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let state = extension_collections(
        respond_or_return!(
            responder,
            account_store
                .mutate(|store| store.delete_collection(&request.collection_id))
                .map_err(agent_error)
        )
        .state,
    );
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(CollectionStateResponse { state })
    })
}

pub(super) fn assign_thread_collection(
    context: &ServerContext,
    request: &extension::AssignThreadCollectionRequest,
    responder: Responder<extension::AssignThreadCollectionResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Collections) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let Some(store) = &ctx_store else {
        return responder.respond_with_internal_error("collections are unavailable");
    };
    let thread_id = respond_or_return!(
        responder,
        SessionId::from_str(&request.thread_id)
            .map_err(|_| { agent_client_protocol::util::internal_error("invalid thread ID") })
    );
    let account_store = respond_or_return!(
        responder,
        ctx_account_work.register_store(store).map_err(agent_error)
    );
    let (change, revision) = respond_or_return!(
        responder,
        account_store
            .mutate(|store| {
                let change =
                    store.assign_thread_collection(&thread_id, request.collection_id.as_deref())?;
                let revision = store.thread_revision(&thread_id)?;
                Ok((change, revision))
            })
            .map_err(agent_error)
    );
    let state = extension_collections(change.state);
    account_store.publish(|| {
        send_extension_activity(
            connection,
            &ctx_extension_state,
            None,
            ActivityEvent::CollectionsChanged {
                state: state.clone(),
            },
        )?;
        responder.respond(extension::AssignThreadCollectionResponse {
            state,
            thread_revision: revision.revision,
            last_timeline_sequence: revision.last_timeline_sequence,
        })
    })
}
