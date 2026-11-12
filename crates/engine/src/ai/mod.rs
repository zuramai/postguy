//! The AI Agent's provider client: one non-streaming completion per call, through the same
//! HTTP client (proxy, TLS, CA bundle) as every other send. The API key comes from the key
//! store (the OS keychain by default); neither the key nor the headers are ever logged, and a
//! completion leaves no console or history entry.

mod wire;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use postguy_http::{Cancel, RequestSpec};
use postguy_model::{AgentChat, AiProvider, ChatMessage, RequestSettings, ToolCall};
use postguy_store::keychain::{self, KeyStore};
use serde_json::Value;

use crate::error::{EngineError, Result};
use crate::{Engine, panic_message};

/// The whole-request timeout of one completion.
const COMPLETION_TIMEOUT_MS: u64 = 180_000;
/// The largest provider reply read.
const COMPLETION_BODY_CAP: usize = 8 << 20;

/// A tool offered to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct AiTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object.
    pub input_schema: Value,
}

/// One model reply: text, tool calls, or both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

impl Engine {
    fn key_store(&self) -> Arc<dyn KeyStore> {
        self.inner.key_store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Replace the key store (tests and embedders; the default is the OS keychain).
    pub fn set_key_store(&self, store: Arc<dyn KeyStore>) {
        *self.inner.key_store.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = store;
    }

    /// The stored API key of `provider`. Call from a worker thread: the OS keychain may block.
    pub fn ai_api_key(&self, provider: AiProvider) -> Result<Option<String>> {
        Ok(self.key_store().get(&keychain::ai_account(provider))?)
    }

    /// Store (`Some`, trimmed) or remove (`None`) the API key of `provider`.
    pub fn set_ai_api_key(&self, provider: AiProvider, key: Option<&str>) -> Result<()> {
        let account = keychain::ai_account(provider);
        match key.map(str::trim) {
            Some("") => Err(EngineError::BadRequest { reason: "the API key is empty".to_string() }),
            Some(key) => Ok(self.key_store().set(&account, key)?),
            None => Ok(self.key_store().delete(&account)?),
        }
    }

    /// The persisted agent chat of this workspace.
    pub fn agent_chat(&self) -> Result<AgentChat> {
        Ok(postguy_store::agent_chat::load(self.store_paths())?)
    }

    /// Persist the agent chat atomically (capped by the store).
    pub fn save_agent_chat(&self, chat: &AgentChat) -> Result<()> {
        Ok(postguy_store::agent_chat::save(self.store_paths(), chat)?)
    }

    /// Ask the provider chosen in Settings → AI Agent for the next turn. Blocks the caller;
    /// `cancel` aborts the request.
    pub fn ai_complete(&self, api_key: &str, system: &str, messages: &[ChatMessage], tools: &[AiTool], cancel: &Cancel) -> Result<AiTurn> {
        let settings = self.settings();
        let ai = &settings.ai;
        let wire = wire::build(ai, api_key, system, messages, tools)?;
        let body = serde_json::to_vec(&wire.body).map_err(|err| EngineError::Ai { message: format!("cannot encode the request: {err}") })?;
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), format!("Postguy/{}", env!("CARGO_PKG_VERSION"))),
        ];
        headers.extend(wire.headers);
        let spec = RequestSpec {
            method: hyper::Method::POST,
            url: wire.url,
            headers,
            body,
            timeout_ms: COMPLETION_TIMEOUT_MS,
            max_body_bytes: COMPLETION_BODY_CAP,
            decode_compression: true,
        };
        let client = self.client_for(&RequestSettings::default(), &settings)?;
        let response = match std::panic::catch_unwind(AssertUnwindSafe(|| self.inner.runtime.block_on(client.send(spec, cancel)))) {
            Ok(response) => response?,
            Err(payload) => return Err(EngineError::Panicked { message: panic_message(&payload) }),
        };
        wire::parse(ai.provider, response.status.as_u16(), &response.body)
    }
}
