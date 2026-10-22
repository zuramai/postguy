//! Export values for the UI dialogs and automation tools: Postman collection v2.1,
//! native collection JSON, environment/globals and the all-data zip. Secret variable values are
//! blanked unless the caller explicitly asks to include them; the Postman schemas keep the
//! `secret` type flag either way.

use postguy_model::{Collection, Item, Variable};
use serde::Serialize;
use serde_json::Value;

use crate::Engine;
use crate::error::{EngineError, Result};

/// The collection export shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionFormat {
    Postman,
    Native,
}

impl Engine {
    /// Export one collection as a Postman v2.1 document or the native workspace shape.
    pub fn export_collection_value(&self, collection: &postguy_model::Id, format: CollectionFormat) -> Result<Value> {
        let mut collection = self.collection(collection)?;
        strip_collection_secrets(&mut collection);
        Ok(match format {
            CollectionFormat::Postman => postguy_interop::export_collection(&collection),
            CollectionFormat::Native => serde_json::to_value(&collection)
                .map_err(|err| EngineError::Import { message: format!("serializing the native collection: {err}") })?,
        })
    }

    /// Export one environment in the Postman environment shape.
    pub fn export_environment_value(&self, environment: &postguy_model::Id, include_secrets: bool) -> Result<Value> {
        let mut environment = self.environment(environment)?;
        if !include_secrets {
            blank_secrets(&mut environment.variables);
        }
        Ok(postguy_interop::export_environment(&environment, "environment"))
    }

    /// Export globals in the Postman globals shape.
    pub fn export_globals_value(&self, include_secrets: bool) -> Result<Value> {
        let mut globals = self.globals()?;
        if !include_secrets {
            blank_secrets(&mut globals.variables);
        }
        Ok(postguy_interop::export_environment(&globals, "globals"))
    }
}

fn blank_secrets(variables: &mut [Variable]) {
    for variable in variables.iter_mut().filter(|variable| variable.secret) {
        variable.value.clear();
    }
}

/// Collection and folder variables can be secret; walk iteratively and blank them.
fn strip_collection_secrets(collection: &mut Collection) {
    blank_secrets(&mut collection.variables);
    strip_oauth2_option(&mut collection.auth);
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    let mut budget = 500_000usize;
    while let Some(item) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        match item {
            Item::Folder(folder) => {
                blank_secrets(&mut folder.variables);
                strip_oauth2_option(&mut folder.auth);
                stack.extend(folder.items.iter_mut());
            }
            Item::Request(request) => {
                if let postguy_model::RequestKind::Http(http) = &mut request.kind {
                    strip_oauth2(&mut http.auth);
                }
            }
        }
    }
}

fn strip_oauth2_option(auth: &mut Option<postguy_model::AuthConfig>) {
    if let Some(auth) = auth {
        strip_oauth2(auth);
    }
}

fn strip_oauth2(auth: &mut postguy_model::AuthConfig) {
    if let postguy_model::AuthConfig::OAuth2(oauth2) = auth {
        oauth2.access_token.clear();
        oauth2.refresh_token.clear();
        oauth2.access_token_expires_at_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_are_blanked_but_the_flag_survives() {
        let mut collection = Collection::default();
        collection.variables.push(Variable { key: "token".into(), value: "hunter2".into(), secret: true, ..Variable::default() });
        collection.items.push(Item::Folder(Box::new(postguy_model::Folder {
            variables: vec![Variable { key: "nested".into(), value: "secret".into(), secret: true, ..Variable::default() }],
            ..postguy_model::Folder::default()
        })));
        strip_collection_secrets(&mut collection);
        assert_eq!(collection.variables[0].value, "");
        assert!(collection.variables[0].secret);
        let Item::Folder(folder) = &collection.items[0] else { panic!("folder") };
        assert_eq!(folder.variables[0].value, "");
    }
}
