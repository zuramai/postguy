/// Why a run could not be planned or its data could not be read. Messages are for people and
/// agents: they say what was wrong and what is allowed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    #[error("{0}")]
    BadConfig(String),
    #[error("{0} was not found in this collection")]
    NotFound(String),
    #[error("{0}")]
    Data(String),
}
