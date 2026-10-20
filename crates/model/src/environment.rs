//! Environments and globals variables (PROMPT.md 4.5, 5.2). Secret values never live in the
//! collection or environment FILE on anywhere but the secrets store; `Variable` carries only
//! the flag — the value split is `store`'s job (secret values live `.postguy/secrets.json`).

use crate::extras::Extras;
use crate::ids::Id;
use serde::{Deserialize, Serialize};

/// One variable row: key, value, description, enabled, secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "true_value")]
    pub enabled: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

fn true_value() -> bool {
    true
}

/// An environment (and globals, which are an environment file by another name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Default for Variable {
    /// A new row is enabled, matching the file default (`enabled` missing means true).
    fn default() -> Self {
        Variable { key: String::new(), value: String::new(), description: String::new(), enabled: true, secret: false, extras: Extras::default() }
    }
}

impl Environment {
    /// Look up the VALUE of an enabled, non-secret-flag-aware variable by key: enabled rows
    /// only (PROMPT.md 5.2: a disabled row does not exist for resolution).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.variables.iter().find(|v| v.enabled && v.key == key).map(|v| v.value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_rows_and_defaults_round_trip() {
        let env: Environment = serde_json::from_str(
            r#"{"id": "e1", "name": "dev", "variables": [
                {"key": "baseUrl", "value": "http://localhost", "omitMe": true},
                {"key": "off", "value": "x", "enabled": false},
                {"key": "noType"}]}"#,
        )
        .unwrap();
        assert_eq!(env.variables.len(), 3);
        // Missing enabled defaults to true (PROMPT.md 5.7: tolerant import).
        assert!(env.variables[0].enabled);
        assert_eq!(env.get("baseUrl"), Some("http://localhost"));
        assert!(!env.variables[2].secret, "defaults off");
        assert_eq!(env.get("off"), None);
        // Unknown key on the variable row preserved.
        assert!(env.variables[0].extras.get("omitMe").is_some());
    }
}
