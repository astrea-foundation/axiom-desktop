//! Bounded tool contracts. Calls stay provisional until authenticated completion.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const MAX_TOOLS: usize = 128;
pub const MAX_ARGUMENTS: usize = 262_000;

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

pub fn definitions(values: &[Value]) -> Result<BTreeSet<String>> {
    ensure!(values.len() <= MAX_TOOLS, "tool count limit");
    let mut names = BTreeSet::new();
    for value in values {
        let tool: ToolDefinition = crate::strict::decode(&serde_json::to_vec(value)?)?;
        ensure!(
            tool.kind == "function"
                && name(&tool.function.name)
                && tool.function.description.len() <= 16_384
                && tool.function.parameters.is_object()
                && serde_json::to_vec(&tool.function.parameters)?.len() <= 131_072
                && names.insert(tool.function.name),
            "invalid or duplicate tool definition"
        );
    }
    Ok(names)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}
impl ToolCall {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.kind == "function"
                && valid_id(&self.id)
                && name(&self.function.name)
                && self.function.arguments.len() <= MAX_ARGUMENTS,
            "invalid tool call"
        );
        Ok(())
    }
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn contracts_are_closed_bounded_and_unique() {
        let tool = json!({"type":"function","function":{"name":"generate","description":"draw","parameters":{"type":"object"}}});
        assert!(definitions(&[tool.clone()]).unwrap().contains("generate"));
        assert!(definitions(&[tool.clone(), tool.clone()]).is_err());
        let mut unknown = tool;
        unknown["url"] = json!("http://untrusted");
        assert!(definitions(&[unknown]).is_err());
        let mut call = ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "generate".into(),
                arguments: "{}".into(),
            },
        };
        assert!(call.validate().is_ok());
        call.function.arguments = "x".repeat(MAX_ARGUMENTS + 1);
        assert!(call.validate().is_err());
    }
}
