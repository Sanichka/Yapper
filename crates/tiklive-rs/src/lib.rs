//! Native, dependency-light TikTok LIVE client.
//!
//! This crate is a Rust port of the zero-login connection flow used by
//! [`talkaboutdesign/tiklive`](https://github.com/talkaboutdesign/tiklive):
//!
//! 1. Plain HTTPS room lookup on `tiktok.com` (anonymous, no signing).
//! 2. Anonymous `ttwid` device cookie harvested from the web page.
//! 3. WebSocket upgrade to `webcast-ws.tiktok.com` with `Cookie: ttwid=...`.
//! 4. Protobuf `PushFrame` / `Webcast*Message` decoding.
//!
//! No login, no `X-Bogus`/`msToken` signing, no third-party signing server.

pub mod connection;
pub mod error;
pub mod events;
pub mod proto;
pub mod web;

pub use connection::{
    ConnectOptions, ConnectionEvent, DisconnectReason, LiveClient, LiveSession, SessionControl,
    DEFAULT_WS_HOSTS,
};
pub use error::{LiveError, Result};
pub use events::{Emote, GiftReceiver, LiveEvent, LiveUser, RoomOwner, RoomStatus, TopViewer};
pub use web::normalize_username;
