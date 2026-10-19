//! `extras`: every model struct keeps its unknown JSON fields (PROMPT.md 4.5, AGENTS fidelity
//! rule) and writes them back on export, so a file written by a newer Postguy import stays a
//! file by a newer Postguy.

use serde::{Deserialize, Serialize};

/// The unknown fields of one struct, keyed exactly as they appeared in the file.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Extras(pub serde_json::Map<String, serde_json::Value>);

impl Extras {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Document-order iteration, for reports and imports.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &serde_json::Value)> {
        self.0.iter()
    }

    /// Look up one unknown field by its exact key.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattened_extras_survive_a_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        #[serde(rename_all = "camelCase")]
        struct Probe {
            pub name: String,
            #[serde(default, flatten)]
            pub extras: Extras,
        }

        // Unknown origin keys and an empty default both round trip exactly.
        let input = r#"{"name":"probe","customField":42,"future":{"nested":true}}"#;
        let parsed: Probe = serde_json::from_str(input).expect("parses");
        assert_eq!(parsed.extras.len(), 2);
        let output = serde_json::to_string(&parsed).expect("serializes");
        let reparsed: Probe = serde_json::from_str(&output).expect("re-parses");
        assert_eq!(parsed, reparsed);
        assert_eq!(reparsed.extras.get("customField"), Some(&json!(42)));
    }

    #[test]
    fn extras_default_round_trips_without_noise() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        #[serde(rename_all = "camelCase")]
        struct Probe {
            pub name: String,
            #[serde(default, flatten)]
            pub extras: Extras,
        }
        let probe = Probe { name: "plain".into(), extras: Extras::default() };
        let out = serde_json::to_value(&probe).expect("serializes");
        assert_eq!(out, json!({"name": "plain"}), "no extras key leak");
    }
}
