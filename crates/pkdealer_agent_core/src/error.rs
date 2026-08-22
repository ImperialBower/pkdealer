//! Error type for agent operations.

use std::fmt;

/// Errors that can occur while running a poker agent.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::AgentError;
///
/// let e = AgentError::Seat("no seats available".to_string());
/// assert!(e.to_string().contains("seat failed"));
/// ```
#[derive(Debug)]
pub enum AgentError {
    /// Failed to establish a gRPC connection.
    Connect(tonic::transport::Error),
    /// The service rejected the seat request.
    Seat(String),
    /// A gRPC RPC call returned a non-OK status.
    ///
    /// Boxed because `tonic::Status` is 176 bytes — inlining it would make
    /// every `Result<T, AgentError>` in this crate that wide on the success
    /// path too (`clippy::result_large_err`). Use `?` and the
    /// `From<tonic::Status>` impl below; the box is invisible at call sites.
    Rpc(Box<tonic::Status>),
    /// A player token could not be parsed as gRPC metadata.
    InvalidMetadata(tonic::metadata::errors::InvalidMetadataValue),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "connection failed: {e}"),
            Self::Seat(msg) => write!(f, "seat failed: {msg}"),
            Self::Rpc(status) => write!(f, "gRPC error: {status}"),
            Self::InvalidMetadata(e) => write!(f, "invalid metadata value: {e}"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) => Some(e),
            Self::InvalidMetadata(e) => Some(e),
            _ => None,
        }
    }
}

impl From<tonic::transport::Error> for AgentError {
    fn from(e: tonic::transport::Error) -> Self {
        Self::Connect(e)
    }
}

impl From<tonic::Status> for AgentError {
    fn from(s: tonic::Status) -> Self {
        Self::Rpc(Box::new(s))
    }
}

impl From<tonic::metadata::errors::InvalidMetadataValue> for AgentError {
    fn from(e: tonic::metadata::errors::InvalidMetadataValue) -> Self {
        Self::InvalidMetadata(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_error_seat_display() {
        let e = AgentError::Seat("all seats taken".to_string());
        assert_eq!(e.to_string(), "seat failed: all seats taken");
    }

    #[test]
    fn agent_error_rpc_display() {
        let status = tonic::Status::not_found("table not found");
        let e = AgentError::Rpc(Box::new(status));
        assert!(e.to_string().contains("gRPC error"));
    }

    #[test]
    fn agent_error_seat_debug() {
        let e = AgentError::Seat("err".to_string());
        assert!(format!("{e:?}").contains("Seat"));
    }

    #[test]
    fn from_tonic_status() {
        let status = tonic::Status::internal("oops");
        let e: AgentError = status.into();
        assert!(matches!(e, AgentError::Rpc(_)));
    }

    #[test]
    fn error_source_seat_is_none() {
        let e = AgentError::Seat("x".to_string());
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn error_source_rpc_is_none() {
        let e = AgentError::Rpc(Box::new(tonic::Status::ok("")));
        assert!(std::error::Error::source(&e).is_none());
    }
}
