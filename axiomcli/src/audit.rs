use serde_json::Value;

const SECRET_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "encryption_key",
    "password",
    "private_key",
    "secret",
    "token",
];

/// Redact structured secret fields and recognizable credential prefixes before
/// data crosses the audit/storage boundary. This is intentionally conservative.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if SECRET_KEYS
                    .iter()
                    .any(|secret| key.to_ascii_lowercase().contains(secret))
                {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_value(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_value),
        Value::String(text) => *text = redact_text(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[must_use]
pub fn redact_text(input: &str) -> String {
    let mut redact_next = false;
    input
        .split_inclusive(char::is_whitespace)
        .map(|piece| {
            let token = piece.trim_end_matches(char::is_whitespace);
            let whitespace = &piece[token.len()..];
            let redact = redact_next || looks_like_secret(token);
            redact_next = token.eq_ignore_ascii_case("bearer");
            if redact {
                format!("[REDACTED]{whitespace}")
            } else {
                piece.to_owned()
            }
        })
        .collect()
}

fn looks_like_secret(token: &str) -> bool {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | ',' | ';'
        )
    });
    ["axm_", "sk-", "ghp_", "github_pat_"]
        .iter()
        .any(|prefix| token.starts_with(prefix) && token.len() > prefix.len() + 8)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn structured_and_inline_secrets_are_removed() {
        let mut value = json!({
            "authorization": "Bearer sensitive",
            "nested": {"message": "use axm_123456789abcdef and Bearer abcdefghijklmnop now"},
            "safe": "cherry"
        });
        redact_value(&mut value);
        let serialized = value.to_string();
        assert!(!serialized.contains("sensitive"));
        assert!(!serialized.contains("axm_"));
        assert!(!serialized.contains("abcdefghijklmnop"));
        assert!(serialized.contains("cherry"));
    }
}
