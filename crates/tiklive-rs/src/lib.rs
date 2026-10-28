//! Native, dependency-light TikTok LIVE client.
//!
//! Connection flow:
//!
//! 1. Plain HTTPS room lookup on `tiktok.com`.
//! 2. Ask EulerStream's public webcast endpoint for a signed room handshake.
//! 3. Open `webcast-ws.tiktok.com` with the returned route parameters and
//!    anonymous cookie.
//! 4. Decode the initial batch and subsequent protobuf `PushFrame` messages.
//!
//! No login is required. Live connections use EulerStream for request signing.

pub mod connection;
pub mod error;
pub mod events;
pub mod proto;
pub mod web;

pub use connection::{
    ConnectOptions, ConnectionEvent, DisconnectReason, ErrorKind, LiveClient, LiveSession,
    SessionControl, DEFAULT_WS_HOSTS,
};
pub use error::{LiveError, Result};
pub use events::{Emote, GiftReceiver, LiveEvent, LiveUser, RoomOwner, RoomStatus, TopViewer};
pub use web::normalize_username;
