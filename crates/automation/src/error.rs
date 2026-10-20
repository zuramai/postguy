//! Error vocabulary for the tool layer: every message says what was wrong AND what is allowed
//! (AGENTS.md 3), and path confinement uses one identical refusal.

/// Result alias for the automation surface.
pub type Result<T, E = AutomationError> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum AutomationError {
    /// A path resolved outside the allowed root. `display` is the path AS GIVEN by the caller;
    /// the message is IDENTICAL for existing and missing paths so an agent cannot probe.
    OutsideRoot { path: String, root: String },
    /// No workspace is open (the tools need `workspace_open` first).
    NoWorkspace,
    /// An argument name the tool does not accept; names the accepted set.
    UnknownArgument { tool: String, argument: String, allowed: Vec<String> },
    /// A required argument was absent.
    MissingArgument { tool: String, argument: String },
    /// The argument had the wrong JSON shape.
    BadArgument { tool: String, argument: String, expected: String, got: String },
    /// A workspace entity was not found by id.
    NotFound { kind: String, id: String },
    /// Store-level failure, already human readable.
    Store { message: String },
    /// Import/export failure, already human readable.
    Interop { message: String },
    /// The tool name is not registered.
    UnknownTool { tool: String },
    /// A failure with a ready-to-read message.
    Failed { message: String },
}

impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutomationError::OutsideRoot { path, root } => {
                write!(f, "{path} is outside the allowed directory {root}")
            }
            AutomationError::NoWorkspace => f.write_str("no workspace is open; call workspace_open first"),
            AutomationError::UnknownArgument { tool, argument, allowed } => {
                write!(f, "{tool}: unknown argument `{argument}`; allowed: {}", allowed.join(", "))
            }
            AutomationError::MissingArgument { tool, argument } => write!(f, "{tool}: missing required argument `{argument}`"),
            AutomationError::BadArgument { tool, argument, expected, got } => {
                write!(f, "{tool}: argument `{argument}` must be {expected}, got {got}")
            }
            AutomationError::NotFound { kind, id } => write!(f, "{kind} `{id}` not found in the workspace"),
            AutomationError::Store { message } => write!(f, "workspace: {message}"),
            AutomationError::Interop { message } => write!(f, "import/export: {message}"),
            AutomationError::UnknownTool { tool } => write!(f, "unknown tool `{tool}`; run `postguy-cli tools`"),
            AutomationError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for AutomationError {}

impl From<postguy_util::Error> for AutomationError {
    fn from(err: postguy_util::Error) -> AutomationError {
        AutomationError::Store { message: err.to_string() }
    }
}

impl From<postguy_engine::EngineError> for AutomationError {
    fn from(err: postguy_engine::EngineError) -> AutomationError {
        AutomationError::Failed { message: err.to_string() }
    }
}

impl From<postguy_store::workspace_file::WorkspaceError> for AutomationError {
    fn from(err: postguy_store::workspace_file::WorkspaceError) -> AutomationError {
        AutomationError::Store { message: err.to_string() }
    }
}
