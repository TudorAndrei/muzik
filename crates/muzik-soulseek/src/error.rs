//! Error mapping from `soulseek_rs::SoulseekRs` to library errors.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    #[error("not connected to the Soulseek server")]
    NotConnected,
    #[error("Soulseek login failed")]
    AuthenticationFailed,
    #[error("network error: {0}")]
    Network(String),
    #[error("operation timed out")]
    Timeout,
    #[error("connection closed")]
    ConnectionClosed,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("job has not finished yet")]
    JobNotFinished,
    #[error("job failed: {0}")]
    JobFailed(String),
    #[error("job was cancelled")]
    JobCancelled,
}

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
