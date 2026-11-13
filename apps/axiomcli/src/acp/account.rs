//! ACP account request handlers.

use super::{
    ActivityEvent, BillingStatusRequest, BillingStatusResponse, CancellationToken, Client,
    ConnectionTo, ExtensionFeature, LogoutRequest, NativeLoginCancelRequest,
    NativeLoginCompleteRequest, NativeLoginStartRequest, NativeLoginStartResponse,
    NativeLoginState, Responder, ServerContext, ValidationStatus, VersionedValidationStatus,
    agent_error, auth_login_method, await_request_future, begin_account_switch,
    commit_account_store_switch, extension, extension_account_status, extension_billing_status,
    extension_enabled, extension_not_negotiated, publish_account_switch_started,
    send_extension_activity, validation_status_if_current,
};

pub(super) fn account_status(
    context: &ServerContext,
    _request: extension::AccountStatusRequest,
    responder: Responder<extension::AccountStatusResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_transition = context.account_transition.clone();
    let ctx_account_work = context.account_work.clone();
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let task_connection = connection.clone();
    connection.spawn(async move {
        let connection = task_connection;
        if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
            return responder.respond_with_error(extension_not_negotiated());
        }
        let _account_transition = ctx_account_transition.lock().await;
        let request_cancellation = responder.cancellation();
        let validation = respond_or_return!(
            responder,
            await_request_future(&request_cancellation, ctx_auth.validate_with_revision(),).await
        );
        let mut validation = validation_status_if_current(&ctx_auth, validation);
        let switch_target = match (&validation.status, &ctx_store) {
            (ValidationStatus::Valid(account), Some(store))
                if store.active_account_id().as_deref() != Some(account.account.id.as_str()) =>
            {
                Some(Some(account.account.id.clone()))
            }
            (ValidationStatus::Missing | ValidationStatus::Expired, Some(store))
                if store.is_active() =>
            {
                Some(None)
            }
            _ => None,
        };
        if let Some(target) = &switch_target {
            respond_or_return!(
                responder,
                begin_account_switch(&ctx_sessions, &ctx_account_work, None, None,)
                    .await
                    .map_err(agent_error)
            );
            publish_account_switch_started(&connection, &ctx_extension_state, &ctx_auth).await?;
            respond_or_return!(
                responder,
                commit_account_store_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    ctx_store.as_ref(),
                    target.as_deref(),
                )
                .await
                .map_err(agent_error)
            );
            validation =
                validation_status_if_current(&ctx_auth, ctx_auth.validate_with_revision().await);
        }
        let status = extension_account_status(validation);
        if switch_target.is_some() {
            send_extension_activity(
                &connection,
                &ctx_extension_state,
                None,
                ActivityEvent::AccountChanged {
                    status: status.clone(),
                },
            )?;
        }
        responder.respond(extension::AccountStatusResponse { status })
    })?;
    Ok(())
}

pub(super) fn native_login_start(
    context: &ServerContext,
    request: &NativeLoginStartRequest,
    responder: Responder<NativeLoginStartResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let method_hint = request.method_hint;
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    connection.spawn(async move {
        if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
            return responder.respond_with_error(extension_not_negotiated());
        }
        // One desktop process owns at most one pending browser
        // authorization. Superseded capabilities are cancelled at the
        // backend before another approval page is created.
        let superseded = {
            let mut logins = ctx_extension_state.native_logins.lock().await;
            logins.drain().map(|(_, login)| login).collect::<Vec<_>>()
        };
        for superseded_login in superseded {
            let _ = superseded_login.cancel().await;
        }
        let request_cancellation = responder.cancellation();
        let login = respond_or_return!(
            responder,
            await_request_future(
                &request_cancellation,
                ctx_auth.start_native_login(method_hint.map(auth_login_method)),
            )
            .await
            .and_then(|result| result.map_err(agent_error))
        );
        if request_cancellation.is_cancelled() {
            let _ = login.cancel().await;
            return responder.respond_with_error(agent_client_protocol::Error::request_cancelled());
        }
        let login_id = uuid::Uuid::new_v4().to_string();
        let state = NativeLoginState {
            login_id: login_id.clone(),
            user_code: login.user_code().into(),
            authorization_url: login.authorization_url().into(),
            browser_opened: login.browser_opened(),
            expires_at: login.expires_at().into(),
        };
        ctx_extension_state
            .native_logins
            .lock()
            .await
            .insert(login_id, login);
        responder.respond(NativeLoginStartResponse { login: state })
    })?;
    Ok(())
}

pub(super) fn native_login_complete(
    context: &ServerContext,
    request: NativeLoginCompleteRequest,
    responder: Responder<extension::AccountStatusResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_transition = context.account_transition.clone();
    let ctx_account_work = context.account_work.clone();
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    let task_connection = connection.clone();
    connection.spawn(async move {
        let connection = task_connection;
        if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
            return responder.respond_with_error(extension_not_negotiated());
        }
        if request.login_id.is_empty() || request.login_id.len() > extension::MAX_IDENTIFIER_BYTES {
            return responder.respond_with_internal_error("native login ID is invalid");
        }
        let request_cancellation = responder.cancellation();
        if request_cancellation.is_cancelled() {
            return responder.respond_with_error(agent_client_protocol::Error::request_cancelled());
        }
        let login = respond_or_return!(
            responder,
            ctx_extension_state
                .native_logins
                .lock()
                .await
                .remove(&request.login_id)
                .ok_or_else(|| {
                    agent_client_protocol::util::internal_error(
                        "native login is missing, expired, or already completed",
                    )
                })
        );
        let cancellation = CancellationToken::new();
        let completion = login.complete(cancellation.clone());
        tokio::pin!(completion);
        let completion_result = tokio::select! {
            biased;
            account = &mut completion => account,
            () = request_cancellation.cancelled() => {
                cancellation.cancel();
                // NativeLogin owns the persistence commit boundary. Do
                // not drop it after cancellation: it may already be in
                // the uninterruptible credential-save phase.
                completion.await
            }
        };
        let account = respond_or_return!(responder, completion_result.map_err(agent_error));
        let _account_transition = ctx_account_transition.lock().await;
        respond_or_return!(
            responder,
            begin_account_switch(&ctx_sessions, &ctx_account_work, None, None,)
                .await
                .map_err(agent_error)
        );
        publish_account_switch_started(&connection, &ctx_extension_state, &ctx_auth).await?;
        respond_or_return!(
            responder,
            commit_account_store_switch(
                &ctx_sessions,
                &ctx_account_work,
                ctx_store.as_ref(),
                Some(&account.account.id),
            )
            .await
            .map_err(agent_error)
        );
        // login.complete persisted the credential. This is the commit
        // point: finish validation and publish the resulting account
        // state even if request cancellation arrives afterward.
        // Revalidate through the manager so a server-side revocation racing
        // completion still fails closed before the next prompt.
        let committed_account_id = account.account.id.clone();
        let mut validation =
            validation_status_if_current(&ctx_auth, ctx_auth.validate_with_revision().await);
        if matches!(
            &validation.status,
            ValidationStatus::Valid(validated)
                if validated.account.id != committed_account_id
        ) {
            validation.status = ValidationStatus::Unavailable(
                "Axiom returned a different account while validating the completed native session"
                    .into(),
            );
        }
        if !matches!(&validation.status, ValidationStatus::Valid(_)) {
            respond_or_return!(
                responder,
                begin_account_switch(&ctx_sessions, &ctx_account_work, None, None,)
                    .await
                    .map_err(agent_error)
            );
            respond_or_return!(
                responder,
                commit_account_store_switch(
                    &ctx_sessions,
                    &ctx_account_work,
                    ctx_store.as_ref(),
                    None,
                )
                .await
                .map_err(agent_error)
            );
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
        responder.respond(extension::AccountStatusResponse { status })
    })?;
    Ok(())
}

pub(super) fn native_login_cancel(
    context: &ServerContext,
    request: NativeLoginCancelRequest,
    responder: Responder<extension::AccountStatusResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    if request.login_id.is_empty() || request.login_id.len() > extension::MAX_IDENTIFIER_BYTES {
        return responder.respond_with_internal_error("native login ID is invalid");
    }
    connection.spawn(async move {
        let login = ctx_extension_state
            .native_logins
            .lock()
            .await
            .remove(&request.login_id);
        if let Some(login) = login {
            respond_or_return!(responder, login.cancel().await.map_err(agent_error));
        }
        let validation = ctx_auth.validate_with_revision().await;
        let status = extension_account_status(validation_status_if_current(&ctx_auth, validation));
        responder.respond(extension::AccountStatusResponse { status })
    })?;
    Ok(())
}

pub(super) fn logout(
    context: &ServerContext,
    _request: LogoutRequest,
    responder: Responder<extension::AccountStatusResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_transition = context.account_transition.clone();
    let ctx_account_work = context.account_work.clone();
    let ctx_auth = context.auth.clone();
    let ctx_extension_state = context.extension_state.clone();
    let ctx_sessions = context.sessions.clone();
    let ctx_store = context.store.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let task_extension = ctx_extension_state.clone();
    let task_auth = ctx_auth.clone();
    let task_store = ctx_store.clone();
    let task_sessions = ctx_sessions.clone();
    let task_work = ctx_account_work.clone();
    let task_transition = ctx_account_transition.clone();
    let task_connection = connection.clone();
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let task_result: agent_client_protocol::Result<_> = async {
            let _account_transition = task_transition.lock().await;
            begin_account_switch(&task_sessions, &task_work, None, None)
                .await
                .map_err(agent_error)?;
            publish_account_switch_started(&task_connection, &task_extension, &task_auth).await?;
            commit_account_store_switch(&task_sessions, &task_work, task_store.as_ref(), None)
                .await
                .map_err(agent_error)?;
            let cancellation = CancellationToken::new();
            let logout = task_auth.logout_with_cancellation(&cancellation);
            tokio::pin!(logout);
            let logout_result = tokio::select! {
                biased;
                result = &mut logout => result,
                () = request_cancellation.cancelled() => {
                    cancellation.cancel();
                    logout.await
                }
            };
            let revision = logout_result.map_err(agent_error)?;
            // Credential removal is committed; always publish the new state.
            let status = extension_account_status(VersionedValidationStatus {
                status: ValidationStatus::Missing,
                revision,
            });
            send_extension_activity(
                &task_connection,
                &task_extension,
                None,
                ActivityEvent::AccountChanged {
                    status: status.clone(),
                },
            )?;
            Ok(extension::AccountStatusResponse { status })
        }
        .await;
        responder.respond_with_result(task_result)
    })?;
    Ok(())
}

pub(super) fn api_key_list(
    context: &ServerContext,
    _request: extension::ApiKeyListRequest,
    responder: Responder<extension::ApiKeyListResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_billing = context.billing.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let client = ctx_billing.clone();
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _guard = guard;
        let operation = client.api_keys(&cancellation);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            result = &mut operation => result,
            () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
        };
        responder.respond_with_result(result.map_err(agent_error))
    })?;
    Ok(())
}

pub(super) fn api_key_create(
    context: &ServerContext,
    request: extension::ApiKeyCreateRequest,
    responder: Responder<extension::ApiKeyCreatedResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_billing = context.billing.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let client = ctx_billing.clone();
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _guard = guard;
        let operation = client.create_api_key(&request.name, &cancellation);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            result = &mut operation => result,
            () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
        };
        responder.respond_with_result(result.map_err(agent_error))
    })?;
    Ok(())
}

pub(super) fn api_key_revoke(
    context: &ServerContext,
    request: extension::ApiKeyRevokeRequest,
    responder: Responder<extension::ApiKeyRevokeResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_billing = context.billing.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Account) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let client = ctx_billing.clone();
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _guard = guard;
        let operation = client.revoke_api_key(&request.id, &cancellation);
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            result = &mut operation => result,
            () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
        };
        responder.respond_with_result(result.map_err(agent_error))
    })?;
    Ok(())
}

pub(super) fn usage_summary(
    context: &ServerContext,
    request: extension::UsageSummaryRequest,
    responder: Responder<extension::UsageSummaryResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_billing = context.billing.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Usage) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let client = ctx_billing.clone();
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _guard = guard;
        let summary = client.usage_summary(&request, &cancellation);
        tokio::pin!(summary);
        let result = tokio::select! {
            biased;
            result = &mut summary => result,
            () = request_cancellation.cancelled() => {
                cancellation.cancel();
                summary.await
            }
        };
        let summary = respond_or_return!(responder, result.map_err(agent_error));
        responder.respond(extension::UsageSummaryResponse { summary })
    })?;
    Ok(())
}

pub(super) fn billing_status(
    context: &ServerContext,
    _request: BillingStatusRequest,
    responder: Responder<BillingStatusResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let ctx_account_work = context.account_work.clone();
    let ctx_billing = context.billing.clone();
    let ctx_extension_state = context.extension_state.clone();
    if !extension_enabled(&ctx_extension_state, ExtensionFeature::Billing) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let task_extension = ctx_extension_state.clone();
    let task_client = ctx_billing.clone();
    let task_connection = connection.clone();
    let cancellation = CancellationToken::new();
    let account_guard = respond_or_return!(
        responder,
        ctx_account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    connection.spawn(async move {
        let _account_guard = account_guard;
        let status = task_client.status(&cancellation);
        tokio::pin!(status);
        let result = tokio::select! {
            biased;
            result = &mut status => result,
            () = request_cancellation.cancelled() => {
                cancellation.cancel();
                status.await
            }
        };
        let status = respond_or_return!(responder, result.map_err(agent_error));
        let status = extension_billing_status(status, task_extension.next_billing_revision());
        send_extension_activity(
            &task_connection,
            &task_extension,
            None,
            ActivityEvent::BillingChanged {
                status: status.clone(),
            },
        )?;
        responder.respond(BillingStatusResponse { status })
    })?;
    Ok(())
}

pub(super) fn redeem_gift_code(
    context: &ServerContext,
    request: extension::GiftCodeRedeemRequest,
    responder: Responder<extension::GiftCodeRedeemResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    let client = context.billing.clone();
    let state = context.extension_state.clone();
    if !extension_enabled(&state, ExtensionFeature::GiftCodes) {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        context
            .account_work
            .register(cancellation.clone())
            .map_err(agent_error)
    );
    let request_cancellation = responder.cancellation();
    let task_connection = connection.clone();
    connection.spawn(async move {
        let _guard = guard;
        let redemption = client.redeem_gift_code(&request.code, &cancellation);
        tokio::pin!(redemption);
        let result = tokio::select! {
            biased;
            result = &mut redemption => result,
            () = request_cancellation.cancelled() => {
                cancellation.cancel();
                redemption.await
            }
        };
        let receipt = respond_or_return!(responder, result.map_err(agent_error));
        let status = extension_billing_status(receipt.status, state.next_billing_revision());
        send_extension_activity(
            &task_connection,
            &state,
            None,
            ActivityEvent::BillingChanged {
                status: status.clone(),
            },
        )?;
        responder.respond(extension::GiftCodeRedeemResponse {
            credited_microusd: receipt.credited_microusd,
            already_redeemed: receipt.already_redeemed,
            status,
        })
    })?;
    Ok(())
}
