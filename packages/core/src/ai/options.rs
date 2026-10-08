use crate::error::CoreError;
use serde::{Deserialize, Serialize};

/// Cross-backend AI request options. Kept in snake_case so the same JSON shape is
/// shared by the Tauri bridge, the Axum body and the Cloudflare Worker body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIRequestOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default = "default_token_parameter")]
    pub token_parameter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub omit_temperature: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_prompt: Option<String>,
    #[serde(default)]
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

fn default_token_parameter() -> String {
    "max_tokens".to_string()
}

impl Default for AIRequestOptions {
    fn default() -> Self {
        Self {
            max_tokens: None,
            token_parameter: default_token_parameter(),
            temperature: None,
            omit_temperature: false,
            reasoning_effort: None,
            extra_prompt: None,
            retries: 0,
            timeout_seconds: None,
        }
    }
}

impl AIRequestOptions {
    pub fn validate(&self) -> Result<(), CoreError> {
        let valid_effort = self.reasoning_effort.as_deref().map_or(true, |e| {
            ["minimal", "low", "medium", "high", "xhigh", "max", "none"].contains(&e)
        });
        let invalid = !["max_tokens", "max_completion_tokens"].contains(&self.token_parameter.as_str())
            || self.max_tokens.is_some_and(|v| v == 0 || v > 1_000_000)
            || self
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || !valid_effort
            || self.retries > 5
            || self
                .timeout_seconds
                .is_some_and(|v| v == 0 || v > 3600)
            || self
                .extra_prompt
                .as_deref()
                .is_some_and(|p| p.encode_utf16().count() > 20000);

        if invalid {
            return Err(CoreError::Config("Invalid AI options".to_string()));
        }
        Ok(())
    }

    /// Apply these options onto a chat-completions request body.
    /// The system message is located by role rather than assuming index 0.
    pub fn apply(&self, body: &mut serde_json::Value) {
        let Some(obj) = body.as_object_mut() else {
            return;
        };

        if let Some(max_tokens) = self.max_tokens {
            obj.remove("max_tokens");
            obj.remove("max_completion_tokens");
            obj.insert(
                self.token_parameter.clone(),
                serde_json::json!(max_tokens),
            );
        }

        if self.omit_temperature {
            obj.remove("temperature");
        } else if let Some(temperature) = self.temperature {
            obj.insert("temperature".to_string(), serde_json::json!(temperature));
        }

        if let Some(effort) = &self.reasoning_effort {
            obj.insert("reasoning_effort".to_string(), serde_json::json!(effort));
        }

        if let Some(prompt) = &self.extra_prompt {
            let trimmed = prompt.trim();
            if !trimmed.is_empty() {
                let extra = format!(
                    "\n\n## Additional Instructions (user-provided, highest priority)\n{trimmed}\n\n## Follow the required output format above."
                );
                if let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
                    if let Some(system) = messages
                        .iter_mut()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
                    {
                        let original = system
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        system["content"] = serde_json::json!(format!("{original}{extra}"));
                    }
                }
            }
        }
    }
}
