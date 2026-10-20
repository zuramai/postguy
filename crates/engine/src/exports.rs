//! Export values for the UI dialogs and automation tools (M4.3): Postman collection v2.1,
//! native collection JSON, environment/globals and the all-data zip. Secret variable values are
//! blanked unless the caller explicitly asks to include them; the Postman schemas keep the
//! `secret` type flag either way.

use postguy_model::{Collection, Folder, Item, Variable};
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
    let mut stack: Vec<&mut Folder> = Vec::new();
    for item in &mut collection.items {
        if let Item::Folder(folder) = item {
            stack.push(folder);
        }
    }
    let mut budget = 500_000usize;
    while let Some(folder) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        blank_secrets(&mut folder.variables);
        for item in &mut folder.items {
            if let Item::Folder(child) = item {
                stack.push(child);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_are_blanked_but_the_flag_survives() {
        let mut collection = Collection::default();
        collection.variables.push(Variable { key: "token".into(), value: "hunter2".into(), secret: true, ..Variable::default() });
        collection.items.push(Item::Folder(Box::new(Folder {
            variables: vec![Variable { key: "nested".into(), value: "secret".into(), secret: true, ..Variable::default() }],
            ..Folder::default()
        })));
        strip_collection_secrets(&mut collection);
        assert_eq!(collection.variables[0].value, "");
        assert!(collection.variables[0].secret);
        let Item::Folder(folder) = &collection.items[0] else { panic!("folder") };
        assert_eq!(folder.variables[0].value, "");
    }
}
