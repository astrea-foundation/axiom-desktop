//! Private account controls. Gift input never enters the composer or transcript.

use std::{
    fmt::{self, Write as _},
    io::Write as _,
    time::Duration,
};

use base64::Engine as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    style::Style,
    widgets::{Block, Clear, Paragraph, Wrap},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    runtime::UiMessage,
    state::{Overlay, TuiState},
    text::{centered_rect, sanitize_terminal_text},
    theme::Theme,
};
use crate::{
    Result,
    auth::AuthManager,
    billing::{BillingClient, BillingStatus},
};

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct GiftInput(String);

impl fmt::Debug for GiftInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct BillingView {
    pub status: Option<BillingStatus>,
    pub busy: bool,
    pub redeem: bool,
    pub message: Option<String>,
    code: GiftInput,
}

pub(super) enum BillingAction {
    None,
    Close,
    Refresh,
    Redeem(String),
    CopyAddress,
}

impl BillingView {
    pub fn new(redeem: bool) -> Self {
        Self {
            redeem,
            ..Self::default()
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.redeem && !self.busy {
            // Never forward rejected characters, overflow, or multiline paste to the composer.
            self.code.0.extend(
                text.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ' '))
                    .take(64_usize.saturating_sub(self.code.0.len())),
            );
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> BillingAction {
        if key.code == KeyCode::Esc {
            return BillingAction::Close;
        }
        if self.busy {
            return BillingAction::None;
        }
        if self.redeem {
            match key.code {
                KeyCode::Enter if !self.code.0.trim().is_empty() => {
                    return BillingAction::Redeem(std::mem::take(&mut self.code.0));
                }
                KeyCode::Backspace => {
                    self.code.0.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.code.0.clear();
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.paste(&ch.to_string());
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Char('r' | 'R') => return BillingAction::Refresh,
                KeyCode::Char('g' | 'G') => {
                    self.redeem = true;
                    self.message = None;
                }
                KeyCode::Char('c' | 'C') => return BillingAction::CopyAddress,
                _ => {}
            }
        }
        BillingAction::None
    }

    fn body(&self) -> String {
        if self.redeem {
            return format!(
                "Redeem a gift code\n\nPaste or type your code (hidden):\n{}\n\n{}\n\nEnter: redeem · Ctrl+U: clear · Esc: close",
                "*".repeat(self.code.0.len()),
                self.message.as_deref().unwrap_or(if self.busy {
                    "Redeeming…"
                } else {
                    "The code's dollar value will be added to this account."
                })
            );
        }
        let mut body = String::new();
        if let Some(status) = &self.status {
            let _ = writeln!(
                body,
                "Available credit: {}\nTrial: {} · Other credit: {}\nTrial credit is used first.",
                usd(i128::from(status.available_microusd)),
                usd(i128::from(status.trial_microusd)),
                usd(i128::from(status.paid_microusd))
            );
            if status.payment_review_required {
                body.push_str("\nSpending is paused while a deposit is reviewed.\n");
            }
            match &status.payment_account {
                Some(payment) if payment.state == "ready" && payment.valuation_enabled => {
                    body.push_str("\nTop up with Zcash (mainnet ZEC only)\n");
                    body.push_str(payment.address.as_deref().unwrap_or("Address unavailable"));
                    body.push_str(
                        "\n\nC: copy address. Send ZEC from your wallet to this address.\n",
                    );
                    let _ = writeln!(
                        body,
                        "Deposits become USD credit after {} confirmations.",
                        payment
                            .required_confirmations
                            .as_deref()
                            .unwrap_or("the required")
                    );
                    for deposit in payment.deposits.iter().take(3) {
                        let label = if deposit.review_required {
                            "under review"
                        } else {
                            deposit
                                .valuation_status
                                .as_deref()
                                .unwrap_or(&deposit.state)
                        };
                        let _ = writeln!(
                            body,
                            "Recent deposit: {label} ({} confirmations)",
                            deposit.confirmations
                        );
                    }
                }
                Some(payment) if !payment.valuation_enabled => {
                    body.push_str("\nZcash credit conversion is unavailable. Do not send funds.\n");
                }
                Some(_) => body.push_str("\nPreparing your Zcash deposit address…\n"),
                None => {
                    body.push_str(
                        "\nZcash deposits are unavailable. You can redeem a gift code.\n",
                    );
                }
            }
        } else {
            body.push_str("Balance not loaded. Sign in with /login to manage credit.\n");
        }
        if self.busy {
            body.push_str("\nRefreshing…\n");
        }
        if let Some(message) = &self.message {
            let _ = writeln!(body, "\n{message}");
        }
        body.push_str("\nR: refresh · G: redeem gift · Up/Down: scroll · Esc: close");
        sanitize_terminal_text(&body)
    }
}

pub(super) fn usd(micro: i128) -> String {
    let magnitude = micro.unsigned_abs();
    let mut fraction = format!("{:06}", magnitude % 1_000_000);
    while fraction.len() > 2 && fraction.ends_with('0') {
        fraction.pop();
    }
    format!(
        "{}${}.{fraction}",
        if micro < 0 { "-" } else { "" },
        magnitude / 1_000_000
    )
}

pub(super) fn render(frame: &mut Frame<'_>, state: &TuiState, view: &BillingView, theme: Theme) {
    let area = centered_rect(94, 88, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(view.body())
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(Block::bordered().title(" Balance and credit · Esc to close "))
            .wrap(Wrap { trim: false })
            .scroll((state.overlay_scroll, 0)),
        area,
    );
}

pub(super) struct BillingUpdate {
    status: BillingStatus,
    message: Option<String>,
}

pub(super) struct BillingFlow {
    client: BillingClient,
    auth: AuthManager,
    generation: u64,
    cancellation: Option<CancellationToken>,
}

impl BillingFlow {
    pub fn new(auth: &AuthManager) -> Result<Self> {
        Ok(Self {
            auth: auth.clone(),
            client: BillingClient::new(
                auth.account_api_origin().as_str(),
                auth.clone(),
                Duration::from_secs(20),
            )?,
            generation: 0,
            cancellation: None,
        })
    }

    pub fn cancel(&mut self) {
        if let Some(token) = self.cancellation.take() {
            token.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn start(
        &mut self,
        state: &mut TuiState,
        code: Option<String>,
        tx: &mpsc::Sender<UiMessage>,
    ) {
        self.cancel();
        let Some(Overlay::Billing(view)) = &mut state.overlay else {
            return;
        };
        view.busy = true;
        view.message = None;
        let generation = self.generation;
        let account_generation = self.auth.generation();
        let cancellation = CancellationToken::new();
        self.cancellation = Some(cancellation.clone());
        let client = self.client.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let result = if let Some(code) = code {
                client
                    .redeem_gift_code(&code, &cancellation)
                    .await
                    .map(|receipt| BillingUpdate {
                        status: receipt.status,
                        message: Some(format!(
                            "{} {} to this account.",
                            usd(i128::from(receipt.credited_microusd)),
                            if receipt.already_redeemed {
                                "was already credited"
                            } else {
                                "added"
                            }
                        )),
                    })
            } else {
                client
                    .status(&cancellation)
                    .await
                    .map(|status| BillingUpdate {
                        status,
                        message: None,
                    })
            };
            let _ = tx
                .send(UiMessage::BillingFinished {
                    generation,
                    account_generation,
                    result,
                })
                .await;
        });
    }

    pub fn finish(
        &mut self,
        state: &mut TuiState,
        generation: u64,
        account_generation: u64,
        result: Result<BillingUpdate>,
    ) {
        if generation != self.generation {
            return;
        }
        let Some(Overlay::Billing(view)) = &mut state.overlay else {
            return;
        };
        view.busy = false;
        if account_generation != self.auth.generation() {
            **view = BillingView::default();
            view.message = Some(
                "The account changed. Close this screen and sign in again before managing credit."
                    .into(),
            );
            state.overlay_scroll = 0;
            return;
        }
        match result {
            Ok(update) => {
                view.status = Some(update.status);
                view.message = update.message;
                view.redeem = false;
            }
            Err(error) => {
                view.message = Some(format!(
                    "{}{}",
                    sanitize_terminal_text(&error.to_string()),
                    if view.redeem {
                        " Paste the same code to retry safely."
                    } else {
                        ""
                    }
                ));
            }
        }
        state.overlay_scroll = 0;
    }

    pub fn copy_address(state: &mut TuiState) -> Result<()> {
        let Some(Overlay::Billing(view)) = &mut state.overlay else {
            return Ok(());
        };
        if let Some(payment) = view
            .status
            .as_ref()
            .and_then(|status| status.payment_account.as_ref())
            && payment.valuation_enabled
            && payment.state == "ready"
            && let Some(address) = &payment.address
        {
            let encoded = base64::engine::general_purpose::STANDARD.encode(address);
            let mut output = std::io::stdout().lock();
            write!(output, "\x1b]52;c;{encoded}\x07")?;
            output.flush()?;
            view.message =
                Some("Copy requested. Your terminal must allow clipboard access.".into());
        }
        Ok(())
    }
}

impl Drop for BillingFlow {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn queued_result_cannot_update_a_reopened_screen_or_a_changed_account() {
        let directory = tempfile::tempdir().unwrap();
        let paths = crate::paths::AxiomPaths::from_roots(
            directory.path().join("config"),
            directory.path().join("data"),
        );
        let auth =
            AuthManager::new_ephemeral_test("http://127.0.0.1:1", Duration::from_secs(1), &paths)
                .unwrap();
        let mut flow = BillingFlow::new(&auth).unwrap();
        let mut state = TuiState::new(
            directory.path().into(),
            "model".into(),
            crate::app::PermissionProfile::Confirm,
        );
        state.overlay = Some(Overlay::Billing(Box::new(BillingView::new(true))));
        flow.cancel();
        flow.finish(
            &mut state,
            0,
            auth.generation(),
            Err(crate::AxiomError::Cancelled),
        );
        let Some(Overlay::Billing(view)) = &state.overlay else {
            panic!("billing");
        };
        assert!(view.message.is_none());
        let previous = auth.generation();
        auth.logout_async().await.unwrap();
        flow.finish(
            &mut state,
            flow.generation,
            previous,
            Ok(BillingUpdate {
                status: BillingStatus {
                    posted_microusd: 1_000_000,
                    available_microusd: 1_000_000,
                    trial_microusd: 0,
                    paid_microusd: 1_000_000,
                    ledger_sequence: 1,
                    payment_review_required: false,
                    currency: "microUSD".into(),
                    payment_account: None,
                    zec_usd_quote: None,
                },
                message: Some("Credit added".into()),
            }),
        );
        let Some(Overlay::Billing(view)) = &state.overlay else {
            panic!("billing");
        };
        assert!(view.status.is_none());
        assert!(view.message.as_ref().unwrap().contains("account changed"));
    }

    #[test]
    fn private_input_is_masked_bounded_and_consumed_only_by_redemption() {
        let mut view = BillingView::new(true);
        let code = "AXG-ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567";
        view.paste(code);
        assert!(!view.body().contains(code));
        assert!(!format!("{view:?}").contains(code));
        assert!(view.body().contains(&"*".repeat(code.len())));
        let BillingAction::Redeem(submitted) =
            view.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("redeem");
        };
        assert_eq!(submitted, code);
        assert!(view.code.0.is_empty());
        view.busy = true;
        view.paste("MUSTNOTBECOMPOSERINPUT");
        assert!(view.code.0.is_empty());
        assert!(matches!(
            view.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            BillingAction::None
        ));
        assert!(matches!(
            view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            BillingAction::Close
        ));
        view.busy = false;
        view.paste(&"A".repeat(1000));
        assert_eq!(view.code.0.len(), 64);
    }

    #[test]
    fn balance_formats_microdollars_without_rounding_and_preserves_review_block() {
        assert_eq!(usd(-1), "-$0.000001");
        assert_eq!(usd(25_010_000), "$25.01");
        let view = BillingView {
            status: Some(BillingStatus {
                posted_microusd: 25_000_000,
                available_microusd: 0,
                trial_microusd: 0,
                paid_microusd: 25_000_000,
                ledger_sequence: 1,
                currency: "microUSD".into(),
                payment_review_required: true,
                payment_account: None,
                zec_usd_quote: None,
            }),
            ..BillingView::default()
        };
        assert!(view.body().contains("Available credit: $0.00"));
        assert!(view.body().contains("Spending is paused"));
        assert!(view.body().contains("G: redeem gift"));
    }
}
