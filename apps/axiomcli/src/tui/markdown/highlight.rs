use std::sync::OnceLock;

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
};

struct HighlightAssets {
    syntaxes: SyntaxSet,
    theme: Theme,
    light_theme: Theme,
}

static ASSETS: OnceLock<HighlightAssets> = OnceLock::new();

fn assets() -> &'static HighlightAssets {
    ASSETS.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .or_else(|| themes.themes.values().next().cloned())
            .unwrap_or_default();
        HighlightAssets {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            theme,
            light_theme: themes
                .themes
                .get("InspiredGitHub")
                .cloned()
                .unwrap_or_default(),
        }
    })
}

pub(super) fn code_lines(
    body: &str,
    info: &str,
    base_style: Style,
    truecolor: bool,
) -> Vec<Vec<Span<'static>>> {
    let source_lines = logical_lines(body);
    if !truecolor {
        return source_lines
            .into_iter()
            .map(|line| vec![Span::styled(line.to_owned(), base_style)])
            .collect();
    }

    let assets = assets();
    let language = info.split_whitespace().next().unwrap_or_default();
    let syntax = syntax_for_token(&assets.syntaxes, language);
    let light = matches!(base_style.bg, Some(Color::Rgb(r, g, b)) if u16::from(r) + u16::from(g) + u16::from(b) > 384);
    let mut highlighter = HighlightLines::new(
        syntax,
        if light {
            &assets.light_theme
        } else {
            &assets.theme
        },
    );

    source_lines
        .into_iter()
        .map(|line| {
            let input = format!("{line}\n");
            highlighter
                .highlight_line(&input, &assets.syntaxes)
                .map_or_else(
                    |_| vec![Span::styled(line.to_owned(), base_style)],
                    |ranges| {
                        ranges
                            .into_iter()
                            .filter_map(|(style, text)| {
                                let text = text.trim_end_matches(['\r', '\n']);
                                (!text.is_empty()).then(|| {
                                    let mut terminal_style = base_style.fg(Color::Rgb(
                                        style.foreground.r,
                                        style.foreground.g,
                                        style.foreground.b,
                                    ));
                                    let mut modifiers = Modifier::empty();
                                    if style.font_style.contains(FontStyle::BOLD) {
                                        modifiers |= Modifier::BOLD;
                                    }
                                    if style.font_style.contains(FontStyle::ITALIC) {
                                        modifiers |= Modifier::ITALIC;
                                    }
                                    if style.font_style.contains(FontStyle::UNDERLINE) {
                                        modifiers |= Modifier::UNDERLINED;
                                    }
                                    terminal_style = terminal_style.add_modifier(modifiers);
                                    Span::styled(text.to_owned(), terminal_style)
                                })
                            })
                            .collect::<Vec<_>>()
                    },
                )
        })
        .collect()
}

fn logical_lines(body: &str) -> Vec<&str> {
    if body.is_empty() {
        return vec![""];
    }
    let mut lines = body.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push("");
    }
    lines
}

fn syntax_for_token<'a>(
    syntaxes: &'a SyntaxSet,
    token: &str,
) -> &'a syntect::parsing::SyntaxReference {
    let lowercase = token.to_ascii_lowercase();
    let canonical = match lowercase.as_str() {
        "c++" | "cpp" => "cpp",
        "c#" | "csharp" => "cs",
        "js" | "jsx" | "javascript" => "js",
        "ts" | "tsx" | "typescript" => "ts",
        "py" | "python" => "py",
        "rb" | "ruby" => "rb",
        "rs" | "rust" => "rs",
        "sh" | "shell" | "zsh" => "sh",
        other => other,
    };
    syntaxes
        .find_syntax_by_token(canonical)
        .or_else(|| syntaxes.find_syntax_by_extension(canonical))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_code_text_with_and_without_highlighting() {
        for truecolor in [false, true] {
            let lines = code_lines(
                "fn main() {\n    println!(\"hi\");\n}\n",
                "rust",
                Style::default(),
                truecolor,
            );
            let plain = lines
                .iter()
                .map(|line| {
                    line.iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            assert_eq!(plain, ["fn main() {", "    println!(\"hi\");", "}"]);
        }
    }
}
