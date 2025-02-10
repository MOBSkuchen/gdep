use std::fmt;
use git2::{Error, ErrorCode};
use crate::config::ConfigError;

#[derive(Debug, Clone)]
pub enum GdepError {
    LocalRepoNotFound(String),
    RemoteRepoNotFound(String),
    ConfigLoadError(ConfigError),
    BranchInferFailed,
    GitError(String, ErrorCode),

    UpdateErrorRepoAhead(usize),
    UpdateErrorAheadBehind(usize, usize),
    
    UpdateFailed(String, ErrorCode)
}

impl fmt::Display for GdepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GdepError::LocalRepoNotFound(path) => write!(f, "Local repository not found: {}", path),
            GdepError::RemoteRepoNotFound(url) => write!(f, "Remote repository not found: {}", url),
            GdepError::ConfigLoadError(err) => write!(f, "Failed to load configuration: {}", err),
            GdepError::BranchInferFailed => write!(f, "Failed to infer branch"),
            GdepError::GitError(msg, code) => write!(f, "Git error ({:?}): {}", code, msg),
            GdepError::UpdateErrorRepoAhead(ahead) => write!(f, "Update failed: local repo is {} commits ahead", ahead),
            GdepError::UpdateErrorAheadBehind(ahead, behind) => write!(f, "Update failed: local repo is {} ahead, {} behind", ahead, behind),
            GdepError::UpdateFailed(msg, code) => write!(f, "Update failed ({:?}): {}", code, msg),
        }
    }
}

impl std::error::Error for GdepError {}

impl From<ConfigError> for GdepError {
    fn from(value: ConfigError) -> Self {
        GdepError::ConfigLoadError(value)
    }
}

impl From<Error> for GdepError {
    fn from(value: Error) -> Self {
        GdepError::GitError(value.message().to_string(), value.code())
    }
}
