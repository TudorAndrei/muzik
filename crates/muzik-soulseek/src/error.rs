//! Error mapping from `soulseek_rs::SoulseekRs` to library errors.

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
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
}

pub type Result<T, E = BridgeError> = std::result::Result<T, E>;

impl From<String> for BridgeError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for BridgeError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<BridgeError> for String {
    fn from(error: BridgeError) -> Self {
        error.to_string()
    }
}

impl From<soulseek_rs::SoulseekRs> for BridgeError {
    fn from(err: soulseek_rs::SoulseekRs) -> Self {
        match err {
            soulseek_rs::SoulseekRs::NetworkError(e) => Self::Network(e.to_string()),
            soulseek_rs::SoulseekRs::AuthenticationFailed => Self::AuthenticationFailed,
            soulseek_rs::SoulseekRs::ParseError(msg)
            | soulseek_rs::SoulseekRs::InvalidMessage(msg)
            | soulseek_rs::SoulseekRs::CompressionError(msg) => Self::Protocol(msg),
            soulseek_rs::SoulseekRs::Timeout => Self::Timeout,
            soulseek_rs::SoulseekRs::ConnectionClosed => Self::ConnectionClosed,
            soulseek_rs::SoulseekRs::NotConnected => Self::NotConnected,
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
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn wire_errors_map_to_the_matching_bridge_variant() {
        assert!(matches!(
            BridgeError::from(soulseek_rs::SoulseekRs::NotConnected),
            BridgeError::NotConnected
        ));
        assert!(matches!(
            BridgeError::from(soulseek_rs::SoulseekRs::AuthenticationFailed),
            BridgeError::AuthenticationFailed
        ));
        assert!(matches!(
            BridgeError::from(soulseek_rs::SoulseekRs::Timeout),
            BridgeError::Timeout
        ));
        assert!(matches!(
            BridgeError::from(soulseek_rs::SoulseekRs::ParseError("oops".to_string())),
            BridgeError::Protocol(message) if message == "oops"
        ));
    }
}
