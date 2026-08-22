use std::{io, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt as _;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    AxiomError, Result,
    agent::{TurnContext, TurnRunner},
    app::{AppCommand, AppEvent, Origin, PermissionProfile, Runtime, SessionId, TurnId},
    policy::{ApprovalHandler, ApprovalRequest},
    session::SessionStore,
};

const CHERRY: Color = Color::Rgb(255, 77, 109);
const CHERRY_BRIGHT: Color = Color::Rgb(255, 107, 133);
const CHERRY_DEEP: Color = Color::Rgb(138, 31, 48);
const SURFACE: Color = Color::Rgb(24, 24, 27);
const TEXT: Color = Color::Rgb(244, 244, 246);
const MUTED: Color = Color::Rgb(127, 127, 136);
const SUCCESS: Color = Color::Rgb(126, 226, 168);
const WARNING: Color = Color::Rgb(235, 181, 92);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Clone, Debug)]
struct Entry {
    kind: EntryKind,
    text: String,
    collapsed: bool,
}

#[derive(Debug)]
pub struct TuiState {
    cwd: PathBuf,
    model: String,
    profile: PermissionProfile,
    security: &'static str,
    input: String,
    entries: Vec<Entry>,
    running: bool,
    status: String,
    scroll: u16,
    follow_output: bool,
}

impl TuiState {
    #[must_use]
    pub fn new(cwd: PathBuf, model: String, profile: PermissionProfile) -> Self {
        Self {
            cwd,
            model,
            profile,
            security: "UNATTESTED DEV",
            input: String::new(),
            entries: vec![Entry {
                kind: EntryKind::System,
                text: "✿ AxiomCLI is ready. Local actions are controlled by the selected permission profile.".into(),
                collapsed: false,
            }],
            running: false,
            status: "Ready".into(),
            scroll: 0,
            follow_output: true,
        }
    }

    fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::PromptAccepted { text, .. } => self.entries.push(Entry {
                kind: EntryKind::User,
                text: sanitize_terminal_text(text),
                collapsed: false,
            }),
            AppEvent::TurnStarted { .. } => {
                self.running = true;
                self.status = "Working…".into();
            }
            AppEvent::TextDelta { text, .. } => {
                self.append_stream(EntryKind::Assistant, text);
                self.status = "Responding…".into();
            }
            AppEvent::ReasoningDelta { text, .. } => {
                self.append_stream(EntryKind::Reasoning, text);
                self.status = "Reasoning…".into();
            }
            AppEvent::ToolProposed { name, .. } => {
                self.entries.push(Entry {
                    kind: EntryKind::Tool,
                    text: format!("Proposed {name}"),
                    collapsed: false,
                });
                self.status = format!("Preparing {name}…");
            }
            AppEvent::ToolStarted { call_id, name } => {
                self.entries.push(Entry {
                    kind: EntryKind::Tool,
                    text: format!("Running {name} · {call_id}"),
                    collapsed: false,
                });
                self.status = format!("Running {name}…");
            }
            AppEvent::ToolOutput {
                content, truncated, ..
            } => {
                let suffix = if *truncated {
                    "\n… output truncated"
                } else {
                    ""
                };
                self.entries.push(Entry {
                    kind: EntryKind::Tool,
                    text: format!("{}{suffix}", sanitize_terminal_text(content)),
                    collapsed: content.lines().count() > 8,
                });
            }
            AppEvent::ToolCompleted { success, .. } => {
                self.status = if *success {
                    "Tool completed".into()
                } else {
                    "Tool failed".into()
                };
            }
            AppEvent::PermissionRequired { explanation, .. } => {
                self.status = format!("Permission required: {explanation}");
            }
            AppEvent::ProviderStatusChanged { connected, detail } => {
                self.status = if *connected {
                    sanitize_terminal_text(detail)
                } else {
                    format!("Provider unavailable: {}", sanitize_terminal_text(detail))
                };
            }
            AppEvent::PermissionProfileChanged { profile } => self.profile = *profile,
            AppEvent::TurnCompleted { .. } => {
                self.running = false;
                self.status = "Ready".into();
            }
            AppEvent::TurnCancelled { .. } => {
                self.running = false;
                self.status = "Cancelled".into();
            }
            AppEvent::ErrorRaised { message, .. } => {
                self.running = false;
                self.status = "Error".into();
                self.entries.push(Entry {
                    kind: EntryKind::Error,
                    text: sanitize_terminal_text(message),
                    collapsed: false,
                });
            }
            _ => {}
        }
        if self.follow_output {
            self.scroll = u16::MAX;
        }
    }

    fn append_stream(&mut self, kind: EntryKind, text: &str) {
        let text = sanitize_terminal_text(text);
        if let Some(last) = self.entries.last_mut()
            && last.kind == kind
        {
            last.text.push_str(&text);
        } else {
            self.entries.push(Entry {
                kind,
                text,
                collapsed: false,
            });
        }
    }

    fn transcript_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for entry in &self.entries {
            let (marker, color, label) = match entry.kind {
                EntryKind::User => ("›", CHERRY_BRIGHT, "YOU"),
                EntryKind::Assistant => ("◆", TEXT, "AXIOM"),
                EntryKind::Reasoning => ("◇", MUTED, "THINKING"),
                EntryKind::Tool => ("◆", WARNING, "TOOL"),
                EntryKind::System => ("✿", CHERRY, "SYSTEM"),
                EntryKind::Error => ("!", Color::Red, "ERROR"),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker} {label} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if entry.collapsed { "[collapsed]" } else { "" },
                    Style::default().fg(MUTED),
                ),
            ]));
            if entry.collapsed {
                let preview = entry.text.lines().take(3).collect::<Vec<_>>().join("\n");
                lines.extend(preview.lines().map(|line| Line::from(format!("  {line}"))));
                lines.push(Line::from(Span::styled(
                    "  … press Enter in a future viewer to expand",
                    Style::default().fg(MUTED),
                )));
            } else {
                lines.extend(
                    entry
                        .text
                        .lines()
                        .map(|line| Line::from(format!("  {line}"))),
                );
            }
            lines.push(Line::default());
        }
        lines
    }
}

enum UiMessage {
    Event(AppEvent),
    Done(TurnId, Result<()>),
    Approval(ApprovalRequest, oneshot::Sender<bool>),
}

#[derive(Clone)]
struct TuiApproval {
    tx: mpsc::UnboundedSender<UiMessage>,
}

#[async_trait]
impl ApprovalHandler for TuiApproval {
    async fn request(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UiMessage::Approval(request, tx))
            .map_err(|_| AxiomError::Cancelled)?;
        tokio::select! {
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            response = rx => Ok(response.unwrap_or(false)),
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

pub async fn run(
    runner: Arc<dyn TurnRunner>,
    cwd: PathBuf,
    model: String,
    profile: PermissionProfile,
    store: Option<SessionStore>,
) -> Result<()> {
    let cwd = cwd.canonicalize()?;
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let runtime = Runtime::new(512);
    let session_id = SessionId::new();
    let created = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            origin: Origin::Tui,
            profile,
        })
        .await?;
    if let Some(store) = &store {
        store.append_all(&created)?;
    }
    let mut state = TuiState::new(cwd.clone(), model, profile);
    for event in created {
        state.apply(&event.event);
    }

    let mut input_events = EventStream::new();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let mut cancellation: Option<CancellationToken> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<bool>)> = None;

    loop {
        terminal.draw(|frame| render(frame, &state))?;
        tokio::select! {
            maybe_event = input_events.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if let Some((request, _)) = &pending_approval {
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let (_, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(false);
                                    if let Some(token) = &cancellation {
                                        token.cancel();
                                    }
                                    state.status = "Cancelling…".into();
                                }
                                KeyCode::Char('y' | 'Y') => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(true);
                                    state.status = format!("Approved {}", request.request_id);
                                }
                                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(false);
                                    state.status = format!("Denied {}", request.request_id);
                                }
                                _ => {
                                    state.status = format!("{}  [y] allow once  [n] deny", request.explanation);
                                }
                            }
                        } else if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            if let Some(token) = &cancellation {
                                token.cancel();
                                state.status = "Cancelling…".into();
                            } else {
                                break;
                            }
                        } else {
                            match key.code {
                                KeyCode::Esc if !state.running => break,
                                KeyCode::Enter if !state.running && !state.input.trim().is_empty() => {
                                    let prompt = std::mem::take(&mut state.input);
                                    let turn_id = TurnId::new();
                                    let submitted = runtime.dispatch(AppCommand::SubmitPrompt {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        text: prompt.clone(),
                                    }).await?;
                                    if let Some(store) = &store {
                                        store.append_all(&submitted)?;
                                    }
                                    for event in submitted {
                                        state.apply(&event.event);
                                    }
                                    let token = CancellationToken::new();
                                    cancellation = Some(token.clone());
                                    let task_runner = runner.clone();
                                    let task_tx = ui_tx.clone();
                                    let context = TurnContext {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        cwd: cwd.clone(),
                                        permission_profile: profile,
                                        approval: Some(Arc::new(TuiApproval { tx: task_tx.clone() })),
                                    };
                                    tokio::spawn(async move {
                                        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
                                        let forwarding = task_tx.clone();
                                        let forward = tokio::spawn(async move {
                                            while let Some(event) = event_rx.recv().await {
                                                let _ = forwarding.send(UiMessage::Event(event));
                                            }
                                        });
                                        let result = task_runner.run(context, prompt, event_tx, token).await;
                                        let _ = forward.await;
                                        let _ = task_tx.send(UiMessage::Done(turn_id, result));
                                    });
                                }
                                KeyCode::Char(character) if !state.running || key.modifiers.is_empty() => state.input.push(character),
                                KeyCode::Backspace => { state.input.pop(); }
                                KeyCode::Up => {
                                    state.follow_output = false;
                                    state.scroll = state.scroll.saturating_sub(3);
                                }
                                KeyCode::Down => {
                                    state.scroll = state.scroll.saturating_add(3);
                                    if state.scroll == u16::MAX { state.follow_output = true; }
                                }
                                KeyCode::End => { state.follow_output = true; state.scroll = u16::MAX; }
                                _ => {}
                            }
                        }
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            state.follow_output = false;
                            state.scroll = state.scroll.saturating_sub(3);
                        }
                        MouseEventKind::ScrollDown => state.scroll = state.scroll.saturating_add(3),
                        _ => {}
                    },
                    _ => {}
                }
            }
            message = ui_rx.recv() => {
                let Some(message) = message else { break };
                match message {
                    UiMessage::Event(event) => {
                        let envelope = runtime.emit(session_id.clone(), event.clone()).await?;
                        if let Some(store) = &store {
                            store.append(&envelope)?;
                        }
                        state.apply(&event);
                    }
                    UiMessage::Done(turn_id, result) => {
                        pending_approval = None;
                        cancellation = None;
                        let command = match result {
                            Ok(()) => AppCommand::FinishTurn { session_id: session_id.clone(), turn_id },
                            Err(AxiomError::Cancelled) => AppCommand::CancelTurn { session_id: session_id.clone(), turn_id },
                            Err(error) => AppCommand::FailTurn { session_id: session_id.clone(), turn_id, message: error.to_string() },
                        };
                        let completed = runtime.dispatch(command).await?;
                        if let Some(store) = &store {
                            store.append_all(&completed)?;
                        }
                        for event in completed {
                            state.apply(&event.event);
                        }
                    }
                    UiMessage::Approval(request, response) => {
                        state.status = format!("{}  [y] allow once  [n] deny", request.explanation);
                        pending_approval = Some((request, response));
                    }
                }
            }
        }
    }

    let closed = runtime
        .dispatch(AppCommand::CloseSession {
            session_id: session_id.clone(),
        })
        .await?;
    if let Some(store) = &store {
        store.append_all(&closed)?;
    }

    Ok(())
}

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    if area.width < 42 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("AxiomCLI needs at least 42×10 terminal cells.")
                .style(Style::default().fg(CHERRY_BRIGHT))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(frame, state, rows[0]);

    let lines = state.transcript_lines();
    let visible_height = usize::from(rows[1].height.saturating_sub(2));
    let max_scroll = u16::try_from(lines.len().saturating_sub(visible_height)).unwrap_or(u16::MAX);
    let scroll = if state.follow_output || state.scroll == u16::MAX {
        max_scroll
    } else {
        state.scroll.min(max_scroll)
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(CHERRY_DEEP)),
            )
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        rows[1],
    );

    let status_color = if state.running {
        CHERRY_BRIGHT
    } else {
        SUCCESS
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if state.running { "● " } else { "○ " },
                Style::default().fg(status_color),
            ),
            Span::styled(state.status.clone(), Style::default().fg(MUTED)),
        ])),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(state.input.as_str())
            .style(Style::default().fg(TEXT).bg(SURFACE))
            .block(
                Block::default()
                    .title(Span::styled(
                        " › Message ",
                        Style::default().fg(CHERRY_BRIGHT),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if state.running {
                        CHERRY_DEEP
                    } else {
                        CHERRY
                    })),
            ),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new("Enter send  ·  Ctrl+C cancel/quit  ·  ↑↓ scroll  ·  End follow")
            .style(Style::default().fg(MUTED)),
        rows[4],
    );
}

fn render_header(frame: &mut Frame<'_>, state: &TuiState, area: Rect) {
    let cwd = state.cwd.display().to_string();
    let right = if area.width < 72 {
        state.security.into()
    } else {
        format!("{} · {} · {}", state.security, state.model, state.profile)
    };
    let available = usize::from(area.width);
    let left_max = available.saturating_sub(right.chars().count() + 3);
    let left = if area.width < 72 {
        "AXIOMCLI".into()
    } else {
        truncate_middle(&format!("AXIOMCLI · {cwd}"), left_max)
    };
    let gap = " ".repeat(available.saturating_sub(left.chars().count() + right.chars().count()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::raw(gap),
            Span::styled(right, Style::default().fg(CHERRY_BRIGHT)),
        ])),
        area,
    );
}

#[must_use]
pub fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

fn truncate_middle(input: &str, max_chars: usize) -> String {
    let characters: Vec<_> = input.chars().collect();
    if characters.len() <= max_chars {
        return input.into();
    }
    if max_chars <= 1 {
        return "…".into();
    }
    let left = (max_chars - 1) / 2;
    let right = max_chars - left - 1;
    format!(
        "{}…{}",
        characters[..left].iter().collect::<String>(),
        characters[characters.len() - right..]
            .iter()
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn strips_terminal_control_sequences() {
        assert_eq!(sanitize_terminal_text("ok\u{1b}[31mred\u{7}"), "ok[31mred");
    }

    #[test]
    fn renders_at_minimum_and_typical_sizes() {
        for (width, height) in [(42, 10), (100, 30)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let state = TuiState::new(
                PathBuf::from("/tmp/axiom-project"),
                "test-model".into(),
                PermissionProfile::Confirm,
            );
            terminal.draw(|frame| render(frame, &state)).expect("draw");
            let contents = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(contents.contains("AXIOMCLI"));
            assert!(contents.contains("UNATTESTED DEV"));
        }
    }

    #[test]
    fn path_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
    }
}
