//! Random 128-bit ids as text (PROMPT.md 4.5: "ids are UUID text"). The whole workspace uses
//! the constructor, never raw `String` assignment, so an id is never accidentally empty.

use serde::{Deserialize, Serialize};

/// An item id: UUID v4 text. Comparison and hashing are plain string semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub String);

impl Id {
    /// A fresh random id.
    pub fn generate() -> Id {
        Id(new_id())
    }
}

impl Default for Id {
    /// A random id, so a file can never acquire an empty id through `#[serde(default)]`.
    fn default() -> Self {
        Id::generate()
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A new random 128-bit id as text.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique_and_uuid_shaped() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36, "uuid text form is 36 chars: {a}");
        assert_eq!(a.chars().filter(|c| *c == '-').count(), 4);
    }
}
