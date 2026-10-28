use thiserror::Error;

pub type Result<T, E = LiveError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum LiveError {
    #[error("user not found: @{0}")]
    UserNotFound(String),
    #[error("@{0} is not live right now")]
    UserOffline(String),
    #[error("room lookup failed: {0}")]
    RoomLookup(String),
    #[error("TikTok did not issue a ttwid cookie (HTTP {status})")]
    TtwidMissing { status: u16 },
    #[error("TikTok rate limited the request (HTTP {status})")]
    RateLimited { status: u16 },
    #[error("could not sign the TikTok webcast connection: {0}")]
    Signing(String),
    #[error("could not open the TikTok webcast socket: {0}")]
    SocketOpen(String),
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("protocol decode error: {0}")]
    Protocol(String),
    #[error("reconnect failed after {attempts} attempts: {last}")]
    ReconnectFailed { attempts: u32, last: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
}

impl LiveError {
    /// Returns `true` when the error indicates the target user or room is offline.
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::UserOffline(_))
    }

    /// Returns `true` when the error indicates the TikTok user does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::UserNotFound(_))
    }
}

