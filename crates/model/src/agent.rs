//! AI Agent: the provider settings (Settings → AI Agent) and the persisted chat. The API key is
//! never part of these types; it lives in the OS keychain (`store::keychain`).

use crate::extras::Extras;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The model provider the agent talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub enum AiProvider {
    /// Any API that speaks the OpenAI chat-completions format (OpenAI itself by default).
    #[default]
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "openrouter")]
    OpenRouter,
}

impl AiProvider {
    pub const ALL: [AiProvider; 4] = [AiProvider::OpenAiCompatible, AiProvider::Claude, AiProvider::Gemini, AiProvider::OpenRouter];

    /// The stable name used in `settings.json`, tool arguments and keychain accounts.
    pub fn slug(self) -> &'static str {
        match self {
            AiProvider::OpenAiCompatible => "openai-compatible",
            AiProvider::Claude => "claude",
            AiProvider::Gemini => "gemini",
            AiProvider::OpenRouter => "openrouter",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AiProvider::OpenAiCompatible => "OpenAI compatible",
            AiProvider::Claude => "Claude",
            AiProvider::Gemini => "Gemini",
            AiProvider::OpenRouter => "OpenRouter",
        }
    }

    /// An example model id, shown as the field's placeholder. Never used as an implicit default.
    pub fn model_placeholder(self) -> &'static str {
        match self {
            AiProvider::OpenAiCompatible => "gpt-5-mini",
            AiProvider::Claude => "claude-sonnet-4-5",
            AiProvider::Gemini => "gemini-2.5-flash",
            AiProvider::OpenRouter => "deepseek/deepseek-v4.1-flash-1017",
        }
    }

    pub fn from_slug(slug: &str) -> Option<AiProvider> {
        AiProvider::ALL.into_iter().find(|p| p.slug() == slug)
    }
}

/// The model id chosen for each provider, so switching providers keeps every choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModels {
    #[serde(default)]
    pub openai_compatible: String,
    #[serde(default)]
    pub claude: String,
    #[serde(default)]
    pub gemini: String,
    #[serde(default)]
    pub openrouter: String,
}

impl AiModels {
    pub fn get(&self, provider: AiProvider) -> &str {
        match provider {
            AiProvider::OpenAiCompatible => &self.openai_compatible,
            AiProvider::Claude => &self.claude,
            AiProvider::Gemini => &self.gemini,
            AiProvider::OpenRouter => &self.openrouter,
        }
    }

    pub fn get_mut(&mut self, provider: AiProvider) -> &mut String {
        match provider {
            AiProvider::OpenAiCompatible => &mut self.openai_compatible,
            AiProvider::Claude => &mut self.claude,
            AiProvider::Gemini => &mut self.gemini,
            AiProvider::OpenRouter => &mut self.openrouter,
        }
    }
}

pub const DEFAULT_OPENAI_ENDPOINT: &str = "https://api.openai.com/v1";

fn default_openai_endpoint() -> String {
    DEFAULT_OPENAI_ENDPOINT.to_string()
}

/// Settings → AI Agent. Holds no key: the key never touches `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    #[serde(default)]
    pub provider: AiProvider,
    /// Base URL of the OpenAI-compatible API; requests go to `<endpoint>/chat/completions`.
    #[serde(default = "default_openai_endpoint")]
    pub openai_endpoint: String,
    #[serde(default)]
    pub models: AiModels,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            provider: AiProvider::default(),
            openai_endpoint: default_openai_endpoint(),
            models: AiModels::default(),
            extras: Extras::default(),
        }
    }
}

impl AiSettings {
    /// The model chosen for the current provider, trimmed; empty when none is set.
    pub fn model(&self) -> &str {
        self.models.get(self.provider).trim()
    }
}

/// The agent chat of one workspace (`.postguy/agent-chat.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChat {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// One chat entry. `Notice` is local only and never sent to a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatMessage {
    User {
        text: String,
        #[serde(default)]
        time_ms: i64,
    },
    Assistant {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        #[serde(default)]
        time_ms: i64,
    },
    /// A tool result. `content` is the exact text sent back to the model; `label` is the UI step.
    Tool {
        call_id: String,
        name: String,
        content: String,
        ok: bool,
        #[serde(default)]
        label: String,
        #[serde(default)]
        time_ms: i64,
    },
    Notice {
        text: String,
        #[serde(default)]
        error: bool,
        #[serde(default)]
        time_ms: i64,
    },
}

/// A tool call the model asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Provider data that must be echoed back with the call (Gemini's `thoughtSignature`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_data: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    #[test]
    fn ai_settings_default_when_missing_and_unknown_keys_survive() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.ai.provider, AiProvider::OpenAiCompatible);
        assert_eq!(settings.ai.openai_endpoint, "https://api.openai.com/v1");
        assert_eq!(settings.ai.model(), "");

        let settings: Settings = serde_json::from_str(r#"{"ai":{"provider":"claude","future":1}}"#).unwrap();
        assert_eq!(settings.ai.provider, AiProvider::Claude);
        assert_eq!(settings.ai.openai_endpoint, DEFAULT_OPENAI_ENDPOINT);
        let text = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.ai.extras.get("future"), Some(&serde_json::json!(1)));
        assert_eq!(back, settings);
    }

    #[test]
    fn chat_messages_round_trip_with_role_tags() {
        let chat = AgentChat {
            messages: vec![
                ChatMessage::User { text: "hi".into(), time_ms: 1 },
                ChatMessage::Assistant {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "collection_list".into(),
                        arguments: serde_json::json!({}),
                        provider_data: None,
                    }],
                    time_ms: 2,
                },
                ChatMessage::Tool {
                    call_id: "c1".into(),
                    name: "collection_list".into(),
                    content: "{}".into(),
                    ok: true,
                    label: "List".into(),
                    time_ms: 3,
                },
            ],
            extras: Extras::default(),
        };
        let text = serde_json::to_string(&chat).unwrap();
        assert!(text.contains(r#""role":"tool""#) && text.contains(r#""callId":"c1""#) && text.contains(r#""toolCalls""#), "{text}");
        let back: AgentChat = serde_json::from_str(&text).unwrap();
        assert_eq!(back, chat);
    }
}
