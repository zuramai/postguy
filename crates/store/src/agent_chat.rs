//! The AI Agent chat of one workspace: `.postguy/agent-chat.json`. Capped by message count
//! and file size; a corrupt file is quarantined and the chat starts empty.

use postguy_model::{AgentChat, ChatMessage};
use postguy_util::{Result, atomic_write, read_file_capped};

use crate::paths::StorePaths;

/// The most messages kept on disk; older turns are dropped whole.
pub const MAX_MESSAGES: usize = 500;
/// The largest chat file read or written.
pub const FILE_CAP: usize = 8 << 20;

/// Load the chat. A missing file is an empty chat; an unreadable or oversize file is renamed
/// `.corrupt-<time>` and the chat starts empty.
pub fn load(paths: &StorePaths) -> Result<AgentChat> {
    let path = paths.agent_chat_file();
    let bytes = match read_file_capped(&path, FILE_CAP) {
        Ok(bytes) => bytes,
        Err(postguy_util::Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => return Ok(AgentChat::default()),
        Err(postguy_util::Error::TooLarge { .. }) => {
            crate::workspace_file::quarantine(&path)?;
            return Ok(AgentChat::default());
        }
        Err(other) => return Err(other),
    };
    match serde_json::from_slice::<AgentChat>(&bytes) {
        Ok(chat) => Ok(chat),
        Err(_) => {
            crate::workspace_file::quarantine(&path)?;
            Ok(AgentChat::default())
        }
    }
}

/// Save the chat atomically, dropping the oldest turns until it fits [`MAX_MESSAGES`] and
/// [`FILE_CAP`].
pub fn save(paths: &StorePaths, chat: &AgentChat) -> Result<()> {
    let mut chat = chat.clone();
    let mut bytes = serialize(&chat)?;
    while chat.messages.len() > MAX_MESSAGES || bytes.len() > FILE_CAP {
        drop_oldest_turn(&mut chat.messages);
        bytes = serialize(&chat)?;
    }
    atomic_write(&paths.agent_chat_file(), &bytes)
}

fn serialize(chat: &AgentChat) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(chat).map_err(|err| postguy_util::Error::JsonSyntax { what: "serializing the agent chat".to_string(), source: err })
}

/// Remove everything before the second user message, so the chat still starts at a turn.
fn drop_oldest_turn(messages: &mut Vec<ChatMessage>) {
    let second_user = messages.iter().enumerate().filter(|(_, m)| matches!(m, ChatMessage::User { .. })).nth(1).map(|(index, _)| index);
    match second_user {
        Some(index) => {
            messages.drain(..index);
        }
        None => messages.clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_trims_oldest_turns_and_corrupt_files_are_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StorePaths::new(dir.path(), dir.path().join("config"));
        std::fs::create_dir_all(paths.dot_dir()).unwrap();
        let mut chat = AgentChat::default();
        for index in 0..300 {
            chat.messages.push(ChatMessage::User { text: format!("q{index}"), time_ms: 0 });
            chat.messages.push(ChatMessage::Assistant { text: format!("a{index}"), tool_calls: Vec::new(), time_ms: 0 });
        }
        save(&paths, &chat).unwrap();
        let loaded = load(&paths).unwrap();
        assert!(loaded.messages.len() <= MAX_MESSAGES, "{} messages", loaded.messages.len());
        assert!(matches!(loaded.messages.first(), Some(ChatMessage::User { .. })));
        assert_eq!(loaded.messages.last(), chat.messages.last(), "the newest turn is kept");

        std::fs::write(paths.agent_chat_file(), b"{ not json").unwrap();
        assert_eq!(load(&paths).unwrap(), AgentChat::default());
        let quarantined = std::fs::read_dir(paths.dot_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("agent-chat.json.corrupt-"));
        assert!(quarantined, "the corrupt file is kept aside");
    }
}
