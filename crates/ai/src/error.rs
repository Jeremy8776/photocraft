//! The one error type of the port.
//!
//! `kind` lets the host choose behaviour (ask the user to sign in, retry later, say "this model
//! cannot take a mask"); `message` is for people and is safe to show. A backend must never put a
//! secret in either.

use std::fmt;

/// What went wrong, in terms the host can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The backend cannot do this task or input.
    Unsupported,
    /// The request itself is wrong (empty selection, bad size).
    InvalidRequest,
    /// Missing, expired or rejected credentials.
    Auth,
    /// Too many requests; try again later.
    RateLimited,
    /// Out of credits, allowance or budget.
    Quota,
    /// Could not reach the backend.
    Network,
    /// The backend failed or returned something unusable.
    Provider,
    /// The user cancelled.
    Cancelled,
    /// The user has not agreed to send this data to a remote service yet.
    ConsentRequired,
    /// A bug inside the port. Reported, never panicked.
    Internal,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::InvalidRequest => "invalid request",
            Self::Auth => "authentication",
            Self::RateLimited => "rate limited",
            Self::Quota => "quota",
            Self::Network => "network",
            Self::Provider => "provider",
            Self::Cancelled => "cancelled",
            Self::ConsentRequired => "consent required",
            Self::Internal => "internal",
        }
    }
}

/// Error from a backend or from request preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AiError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequest, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "cancelled")
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for AiError {}
