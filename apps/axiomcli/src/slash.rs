//! Typed commands entered through the TUI composer.
//!
//! Slash commands are application controls, not model input. Keeping parsing
//! here prevents command text from accidentally becoming part of a prompt and
//! gives ACP/desktop adapters a stable command vocabulary to mirror later.

use std::{fmt, str::FromStr as _};

use crate::app::ThinkingLevel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Update,
    Usage,
    Permissions,
    Compact { focus: Option<String> },
    Model,
    Resume,
    Delete,
    Thinking(ThinkingLevel),
    Login,
    Logout,
    Account,
    Balance,
    Topup,
    Redeem,
    Security,
    AcceptOutdatedTee,
    Refresh,
    Theme(crate::tui::Appearance),
    Web(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub aliases: &'static [&'static str],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCompletion {
    /// Complete composer contents, not merely the suffix. This keeps aliases
    /// and irregular whitespace from leaking into later parsing.
    pub input: String,
    pub label: String,
    pub description: &'static str,
}

pub const COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "permissions",
        usage: "/permissions",
        description: "Open tool permission settings",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "compact",
        usage: "/compact [what the summary should preserve]",
        description: "Summarize and replace older model context",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "model",
        usage: "/model",
        description: "Open the searchable model picker",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "resume",
        usage: "/resume",
        description: "Open the searchable transcript picker",
        aliases: &["sessions"],
    },
    SlashCommandSpec {
        name: "delete",
        usage: "/delete",
        description: "Select and permanently delete saved transcripts",
        aliases: &["delete-session", "delete-sessions"],
    },
    SlashCommandSpec {
        name: "thinking",
        usage: "/thinking <enabled|disabled|minimal|low|medium|high|xhigh>",
        description: "Set a reasoning mode or effort supported by the selected model",
        aliases: &["reasoning"],
    },
    SlashCommandSpec {
        name: "login",
        usage: "/login",
        description: "Sign in to an Axiom account in your browser",
        aliases: &["signin"],
    },
    SlashCommandSpec {
        name: "logout",
        usage: "/logout",
        description: "Sign out and remove the saved Axiom session from this computer",
        aliases: &["signout"],
    },
    SlashCommandSpec {
        name: "account",
        usage: "/account",
        description: "Check the current Axiom authorization",
        aliases: &["whoami"],
    },
    SlashCommandSpec {
        name: "security",
        usage: "/security [accept-outdated]",
        description: "Inspect attestation, encryption, and workload evidence",
        aliases: &["attestation"],
    },
    SlashCommandSpec {
        name: "refresh",
        usage: "/refresh",
        description: "Run a fresh attestation verification",
        aliases: &["reverify"],
    },
    SlashCommandSpec {
        name: "help",
        usage: "/help",
        description: "Show available slash commands",
        aliases: &["commands"],
    },
    SlashCommandSpec {
        name: "theme",
        usage: "/theme <dark|light|terminal>",
        description: "Choose desktop colors or inherit your terminal appearance",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "web",
        usage: "/web <on|off>",
        description: "Enable external search and URL fetching; queries are not E2EE",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "balance",
        usage: "/balance",
        description: "View credit and Zcash deposits",
        aliases: &["credits"],
    },
    SlashCommandSpec {
        name: "topup",
        usage: "/topup",
        description: "Show your mainnet Zcash deposit address",
        aliases: &["top-up"],
    },
    SlashCommandSpec {
        name: "redeem",
        usage: "/redeem",
        description: "Redeem a gift code in a private entry screen",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "usage",
        usage: "/usage",
        description: "Inspect reported context usage, capacity, and compaction threshold",
        aliases: &[],
    },
    SlashCommandSpec {
        name: "update",
        usage: "/update",
        description: "Install the latest Axiom release and restart this session",
        aliases: &[],
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommandError(String);

impl SlashCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SlashCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SlashCommandError {}

/// Parse an entire composer value. Returns `None` for ordinary prompts.
pub fn parse(input: &str) -> Option<Result<SlashCommand, SlashCommandError>> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    if input == "/" {
        return Some(Ok(SlashCommand::Help));
    }
    let body = &input[1..];
    let (name, arguments) = body
        .split_once(char::is_whitespace)
        .map_or((body, ""), |(name, arguments)| (name, arguments.trim()));
    let canonical = COMMANDS
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name))
        .map(|spec| spec.name);

    Some(match canonical {
        Some("balance") => no_arguments(arguments, SlashCommand::Balance, "/balance"),
        Some("topup") => no_arguments(arguments, SlashCommand::Topup, "/topup"),
        Some("redeem") => no_arguments(
            arguments,
            SlashCommand::Redeem,
            "/redeem (enter the code in the private screen)",
        ),
        Some("update") => no_arguments(arguments, SlashCommand::Update, "/update"),
        Some("help") => no_arguments(arguments, SlashCommand::Help, "/help"),
        Some("usage") => no_arguments(arguments, SlashCommand::Usage, "/usage"),
        Some("permissions") => {
            no_arguments(arguments, SlashCommand::Permissions, COMMANDS[0].usage)
        }
        Some("compact") => Ok(SlashCommand::Compact {
            focus: (!arguments.is_empty()).then(|| arguments.to_owned()),
        }),
        Some("model") => no_arguments(arguments, SlashCommand::Model, COMMANDS[2].usage),
        Some("resume") => no_arguments(arguments, SlashCommand::Resume, COMMANDS[3].usage),
        Some("delete") => no_arguments(arguments, SlashCommand::Delete, COMMANDS[4].usage),
        Some("thinking") => required_argument(arguments, COMMANDS[5].usage).and_then(|value| {
            ThinkingLevel::from_str(value)
                .map(SlashCommand::Thinking)
                .map_err(|_| SlashCommandError::new(COMMANDS[5].usage))
        }),
        Some("login") => no_arguments(arguments, SlashCommand::Login, COMMANDS[6].usage),
        Some("logout") => no_arguments(arguments, SlashCommand::Logout, COMMANDS[7].usage),
        Some("account") => no_arguments(arguments, SlashCommand::Account, COMMANDS[8].usage),
        Some("security") if arguments == "accept-outdated" => Ok(SlashCommand::AcceptOutdatedTee),
        Some("security") => no_arguments(arguments, SlashCommand::Security, COMMANDS[9].usage),
        Some("refresh") => no_arguments(arguments, SlashCommand::Refresh, COMMANDS[10].usage),
        Some("theme") => crate::tui::Appearance::parse(arguments)
            .map(SlashCommand::Theme)
            .ok_or_else(|| SlashCommandError::new("/theme <dark|light|terminal>")),
        Some("web") => match arguments {
            "on" => Ok(SlashCommand::Web(true)),
            "off" => Ok(SlashCommand::Web(false)),
            _ => Err(SlashCommandError::new("/web <on|off>")),
        },
        Some(_) => unreachable!("all command specs are handled"),
        None => Err(SlashCommandError::new(format!(
            "unknown command `/{name}`; use /help to list commands"
        ))),
    })
}

fn no_arguments(
    arguments: &str,
    command: SlashCommand,
    usage: &str,
) -> Result<SlashCommand, SlashCommandError> {
    if arguments.is_empty() {
        Ok(command)
    } else {
        Err(SlashCommandError::new(usage))
    }
}

fn required_argument<'a>(arguments: &'a str, usage: &str) -> Result<&'a str, SlashCommandError> {
    if arguments.is_empty() {
        Err(SlashCommandError::new(usage))
    } else {
        Ok(arguments)
    }
}

/// Command specs matching the command-name fragment currently being typed.
/// Arguments deliberately do not filter the list so usage remains visible.
#[must_use]
pub fn suggestions(input: &str) -> Vec<&'static SlashCommandSpec> {
    let Some(body) = input.strip_prefix('/') else {
        return Vec::new();
    };
    let fragment = body.split_whitespace().next().unwrap_or_default();
    COMMANDS
        .iter()
        .filter(|spec| {
            fragment.is_empty()
                || spec.name.starts_with(fragment)
                || spec.aliases.iter().any(|alias| alias.starts_with(fragment))
        })
        .collect()
}

/// Return context-sensitive completions in deliberate UI order. Permission
/// profiles progress from least to most authority; thinking levels progress
/// from minimal to highest explicit effort.
#[must_use]
pub fn completions(input: &str) -> Vec<SlashCompletion> {
    let Some(body) = input.strip_prefix('/') else {
        return Vec::new();
    };
    let Some((name, arguments)) = body.split_once(char::is_whitespace) else {
        let mut matches = COMMANDS
            .iter()
            .filter(|spec| {
                body.is_empty()
                    || spec.name.starts_with(body)
                    || spec.aliases.iter().any(|alias| alias.starts_with(body))
            })
            .collect::<Vec<_>>();
        // Keep canonical command-name matches ahead of any alias matches.
        matches.sort_by_key(|spec| !spec.name.starts_with(body));
        return matches
            .into_iter()
            .map(|spec| SlashCompletion {
                input: format!(
                    "/{}{}",
                    spec.name,
                    if matches!(
                        spec.name,
                        "help"
                            | "permissions"
                            | "model"
                            | "resume"
                            | "delete"
                            | "login"
                            | "logout"
                            | "account"
                            | "security"
                            | "refresh"
                            | "balance"
                            | "topup"
                            | "redeem"
                            | "usage"
                    ) {
                        ""
                    } else {
                        " "
                    }
                ),
                label: format!("/{}", spec.name),
                description: spec.description,
            })
            .collect();
    };
    let Some(command) = COMMANDS
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name))
        .map(|spec| spec.name)
    else {
        return Vec::new();
    };
    let arguments = arguments.trim_start();
    if arguments.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let options: Vec<(&str, &'static str)> = match command {
        "theme" => vec![
            ("dark", "Charcoal surfaces and orange activity"),
            ("light", "Light surfaces and ink controls"),
            ("terminal", "Inherit terminal foreground and background"),
        ],
        "web" => vec![
            (
                "on",
                "Share search queries and URLs with external services; not E2EE",
            ),
            ("off", "Disable external search and URL fetching"),
        ],
        "thinking" => vec![
            ("enabled", "Enable reasoning"),
            ("disabled", "Disable reasoning"),
            ("minimal", "Use minimal reasoning effort"),
            ("low", "Use low reasoning effort"),
            ("medium", "Use medium reasoning effort"),
            ("high", "Use high reasoning effort"),
            ("xhigh", "Use extra-high reasoning effort"),
        ],
        "permissions" | "compact" | "help" | "update" | "model" | "resume" | "delete" | "login"
        | "logout" | "account" | "security" | "refresh" | "balance" | "topup" | "redeem"
        | "usage" => Vec::new(),
        _ => unreachable!("all command specs are handled"),
    };
    options
        .into_iter()
        .filter(|(value, _)| value.starts_with(arguments))
        .map(|(value, description)| SlashCompletion {
            input: format!("/{command} {value}"),
            label: value.to_owned(),
            description,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_prompts_are_not_commands() {
        assert_eq!(parse("please read /tmp"), None);
    }

    #[test]
    fn parses_every_required_command_and_aliases() {
        assert_eq!(parse("/permissions"), Some(Ok(SlashCommand::Permissions)));
        assert_eq!(
            parse("/compact preserve the failing test names"),
            Some(Ok(SlashCommand::Compact {
                focus: Some("preserve the failing test names".into())
            }))
        );
        assert_eq!(parse("/model"), Some(Ok(SlashCommand::Model)));
        assert_eq!(parse("/resume"), Some(Ok(SlashCommand::Resume)));
        assert_eq!(parse("/sessions"), Some(Ok(SlashCommand::Resume)));
        assert_eq!(parse("/delete"), Some(Ok(SlashCommand::Delete)));
        assert_eq!(parse("/login"), Some(Ok(SlashCommand::Login)));
        assert_eq!(parse("/signout"), Some(Ok(SlashCommand::Logout)));
        assert_eq!(parse("/whoami"), Some(Ok(SlashCommand::Account)));
        assert_eq!(parse("/security"), Some(Ok(SlashCommand::Security)));
        assert_eq!(parse("/attestation"), Some(Ok(SlashCommand::Security)));
        assert_eq!(parse("/refresh"), Some(Ok(SlashCommand::Refresh)));
        assert_eq!(parse("/reverify"), Some(Ok(SlashCommand::Refresh)));
        assert_eq!(
            parse("/reasoning xhigh"),
            Some(Ok(SlashCommand::Thinking(ThinkingLevel::ExtraHigh)))
        );
    }

    #[test]
    fn invalid_commands_fail_closed_instead_of_becoming_prompts() {
        assert!(parse("/wat").expect("slash input").is_err());
        assert!(parse("/permission").expect("slash input").is_err());
        assert!(parse("/mode").expect("slash input").is_err());
        assert!(parse("/model glm-5-2").expect("slash input").is_err());
        assert!(parse("/resume latest").expect("slash input").is_err());
        assert!(parse("/delete latest").expect("slash input").is_err());
        assert!(parse("/permissions confirm").expect("slash input").is_err());
        assert!(parse("/refresh now").expect("slash input").is_err());
        assert!(parse("/thinking enormous").expect("slash input").is_err());
    }

    #[test]
    fn suggestions_match_names_and_aliases() {
        assert_eq!(suggestions("/comp")[0].name, "compact");
        assert_eq!(suggestions("/reas")[0].name, "thinking");
        assert_eq!(suggestions("/").len(), COMMANDS.len());
    }

    #[test]
    fn completions_cover_commands_and_static_options() {
        assert!(completions("/permissions ").is_empty());

        let thinking = completions("/thinking m");
        assert_eq!(
            thinking
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["minimal", "medium"]
        );

        let commands = completions("/per");
        assert_eq!(commands[0].input, "/permissions");
        assert_eq!(completions("/mod")[0].input, "/model");
        assert!(completions("/model ").is_empty());
        assert_eq!(completions("/res")[0].input, "/resume");
        assert_eq!(completions("/del")[0].input, "/delete");
        assert_eq!(completions("/sec")[0].input, "/security");
    }
}
