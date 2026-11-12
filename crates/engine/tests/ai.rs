//! AI provider client against the testkit's scripted OpenAI-compatible endpoint.

use std::sync::Arc;

use postguy_engine::{Cancel, Engine};
use postguy_model::{AiProvider, ChatMessage};
use postguy_store::StorePaths;
use postguy_store::keychain::MemoryKeyStore;
use postguy_testkit::Server;

#[test]
fn ai_complete_posts_to_an_openai_compatible_endpoint() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
    engine.set_key_store(Arc::new(MemoryKeyStore::default()));
    let mut settings = engine.settings();
    settings.ai.provider = AiProvider::OpenAiCompatible;
    settings.ai.openai_endpoint = format!("{}/ai/v1/", server.base_url());
    settings.ai.models.openai_compatible = "fake".to_string();
    engine.update_settings(settings).expect("settings");

    let messages = vec![ChatMessage::User { text: "hi".into(), time_ms: 0 }];
    let turn = engine.ai_complete("test-key", "system", &messages, &[], &Cancel::new()).expect("completion");
    assert_eq!(turn.text, "echo: hi");
    assert!(turn.tool_calls.is_empty());

    let error = engine.ai_complete("wrong", "system", &messages, &[], &Cancel::new()).expect_err("bad key").to_string();
    assert!(error.contains("Incorrect API key provided"), "{error}");
    assert!(error.contains("Settings → AI Agent"), "{error}");
}

#[test]
fn api_keys_are_trimmed_and_an_empty_key_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
    engine.set_key_store(Arc::new(MemoryKeyStore::default()));
    assert_eq!(engine.ai_api_key(AiProvider::Claude).expect("get"), None);
    engine.set_ai_api_key(AiProvider::Claude, Some("  sk-1 \n")).expect("set");
    assert_eq!(engine.ai_api_key(AiProvider::Claude).expect("get").as_deref(), Some("sk-1"));
    assert_eq!(engine.ai_api_key(AiProvider::Gemini).expect("get"), None, "keys are per provider");
    assert!(engine.set_ai_api_key(AiProvider::Claude, Some("   ")).is_err());
    engine.set_ai_api_key(AiProvider::Claude, None).expect("delete");
    assert_eq!(engine.ai_api_key(AiProvider::Claude).expect("get"), None);
}
