//! Error mapping between `soulseek_rs::SoulseekRs` and the Python-facing
//! exception. `BridgeError` stays free of any PyO3 type so it is
//! unit-testable without a Python interpreter.

use pyo3::create_exception;
use pyo3::exceptions::PyException;

create_exception!(_seakarr, SeakarrError, PyException);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    NotConnected,
    AuthenticationFailed,
    Network(String),
    Timeout,
    ConnectionClosed,
    Protocol(String),
    JobNotFinished,
    JobFailed(String),
    JobCancelled,
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "not connected to the Soulseek server"),
            Self::AuthenticationFailed => write!(f, "Soulseek login failed"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::JobNotFinished => write!(f, "job has not finished yet"),
            Self::JobFailed(msg) => write!(f, "job failed: {msg}"),
            Self::JobCancelled => write!(f, "job was cancelled"),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<soulseek_rs::SoulseekRs> for BridgeError {
    fn from(err: soulseek_rs::SoulseekRs) -> Self {
        match err {
            soulseek_rs::SoulseekRs::NetworkError(e) => Self::Network(e.to_string()),
            soulseek_rs::SoulseekRs::AuthenticationFailed => Self::AuthenticationFailed,
            soulseek_rs::SoulseekRs::ParseError(msg) => Self::Protocol(msg),
            soulseek_rs::SoulseekRs::Timeout => Self::Timeout,
            soulseek_rs::SoulseekRs::ConnectionClosed => Self::ConnectionClosed,
            soulseek_rs::SoulseekRs::InvalidMessage(msg) => Self::Protocol(msg),
            soulseek_rs::SoulseekRs::NotConnected => Self::NotConnected,
            soulseek_rs::SoulseekRs::CompressionError(msg) => Self::Protocol(msg),
            soulseek_rs::SoulseekRs::LockPoisoned => {
                Self::Protocol("internal lock poisoned".to_string())
            }
        }
    }
}

impl From<BridgeError> for pyo3::PyErr {
    fn from(err: BridgeError) -> Self {
        SeakarrError::new_err(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_stable_and_non_empty() {
        let cases = [
            BridgeError::NotConnected,
            BridgeError::AuthenticationFailed,
            BridgeError::Network("boom".to_string()),
            BridgeError::Timeout,
            BridgeError::ConnectionClosed,
            BridgeError::Protocol("bad frame".to_string()),
            BridgeError::JobNotFinished,
            BridgeError::JobFailed("peer went offline".to_string()),
            BridgeError::JobCancelled,
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn wire_errors_map_to_the_matching_bridge_variant() {
        assert_eq!(
            BridgeError::from(soulseek_rs::SoulseekRs::NotConnected),
            BridgeError::NotConnected
        );
        assert_eq!(
            BridgeError::from(soulseek_rs::SoulseekRs::AuthenticationFailed),
            BridgeError::AuthenticationFailed
        );
        assert_eq!(
            BridgeError::from(soulseek_rs::SoulseekRs::Timeout),
            BridgeError::Timeout
        );
        assert_eq!(
            BridgeError::from(soulseek_rs::SoulseekRs::ParseError("oops".to_string())),
            BridgeError::Protocol("oops".to_string())
        );
    }
}
