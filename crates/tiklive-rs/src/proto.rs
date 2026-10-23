//! Minimal, forward-compatible webcast protobuf schema.
//!
//! TikTok's `.proto` files are huge and change often. Only the fields this
//! crate reads are declared; `prost` skips everything else, exactly like the
//! dynamic wire decoder used by `tiklive`.

use std::io::Read;

use flate2::read::GzDecoder;
use prost::Message as ProstMessage;

use crate::events::{
    with_envelope_id, Emote, FollowStatus, GiftReceiver, LiveEvent, LiveUser, TopViewer,
};

pub const LIVE_ID: u64 = 12;
pub const WS_PATH: &str = "/webcast/im/ws_proxy/ws_reuse_supplement/";

// ---------------------------------------------------------------------------
// Envelope & transport
// ---------------------------------------------------------------------------

/// Every frame on the webcast WebSocket is a `PushFrame`.
#[derive(Clone, PartialEq, ProstMessage)]
pub struct PushFrame {
    #[prost(uint64, tag = "1")]
    pub seq_id: u64,
    #[prost(uint64, tag = "2")]
    pub log_id: u64,
    #[prost(uint64, tag = "3")]
    pub service: u64,
    #[prost(uint64, tag = "4")]
    pub method: u64,
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<FrameHeader>,
    #[prost(string, tag = "6")]
    pub payload_encoding: String,
    /// `msg`, `im_enter_room_resp`, `hb`, `ack`, ...
    #[prost(string, tag = "7")]
    pub payload_type: String,
    #[prost(bytes = "vec", tag = "8")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct FrameHeader {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// Batch of events pushed over the socket (and returned by `im/fetch`).
#[derive(Clone, PartialEq, ProstMessage)]
pub struct FetchResult {
    #[prost(message, repeated, tag = "1")]
    pub messages: Vec<WebcastMessage>,
    #[prost(string, tag = "2")]
    pub cursor: String,
    #[prost(uint64, tag = "3")]
    pub fetch_interval: u64,
    #[prost(uint64, tag = "4")]
    pub now: u64,
    #[prost(string, tag = "5")]
    pub internal_ext: String,
    #[prost(message, repeated, tag = "7")]
    pub route_params: Vec<RouteParam>,
    #[prost(uint64, tag = "8")]
    pub heartbeat_duration: u64,
    #[prost(bool, tag = "9")]
    pub need_ack: bool,
    #[prost(string, tag = "10")]
    pub push_server: String,
    #[prost(bool, tag = "11")]
    pub is_first: bool,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct RouteParam {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// One raw event. `method` names its schema, e.g. `WebcastChatMessage`.
#[derive(Clone, PartialEq, ProstMessage)]
pub struct WebcastMessage {
    #[prost(string, tag = "1")]
    pub method: String,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub msg_id: u64,
    /// `true` for the handful of comments TikTok replays on join.
    #[prost(bool, tag = "6")]
    pub is_history: bool,
}

pub struct DecodedFrame {
    pub frame: PushFrame,
    pub result: Option<FetchResult>,
}

/// Decodes a WebSocket frame, gunzipping and parsing the event batch.
pub fn decode_socket_frame(buf: &[u8]) -> Result<DecodedFrame, String> {
    let frame = PushFrame::decode(buf).map_err(|e| e.to_string())?;
    if frame.payload_type != "msg" || frame.payload_encoding != "pb" || frame.payload.is_empty() {
        return Ok(DecodedFrame {
            frame,
            result: None,
        });
    }
    let payload = if is_gzip(&frame.payload) {
        gunzip(&frame.payload)
            .map_err(|err| format!("gzip payload could not be decompressed: {err}"))?
    } else {
        frame.payload.clone()
    };
    Ok(DecodedFrame {
        frame,
        result: Some(FetchResult::decode(payload.as_slice()).map_err(|e| e.to_string())?),
    })
}

fn is_gzip(buf: &[u8]) -> bool {
    buf.len() > 2 && buf[0] == 0x1f && buf[1] == 0x8b && buf[2] == 0x08
}

fn gunzip(buf: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(buf).read_to_end(&mut out)?;
    Ok(out)
}

fn frame(payload_type: &str, payload: Vec<u8>) -> PushFrame {
    PushFrame {
        seq_id: 0,
        log_id: 0,
        service: 0,
        method: 0,
        headers: Vec::new(),
        payload_encoding: "pb".into(),
        payload_type: payload_type.into(),
        payload,
    }
}

/// Joins the room on an open socket (mirrors TikTok's web client).
pub fn encode_enter_room(room_id: u64, cursor: &str, enter_unique_id: u64) -> Vec<u8> {
    #[derive(Clone, PartialEq, ProstMessage)]
    struct EnterRoom {
        #[prost(uint64, tag = "1")]
        room_id: u64,
        #[prost(uint64, tag = "4")]
        live_id: u64,
        #[prost(string, tag = "5")]
        identity: String,
        #[prost(string, tag = "6")]
        cursor: String,
        #[prost(uint64, tag = "8")]
        enter_unique_id: u64,
    }
    let body = EnterRoom {
        room_id,
        live_id: LIVE_ID,
        identity: "audience".into(),
        cursor: cursor.into(),
        enter_unique_id,
    };
    frame("im_enter_room", body.encode_to_vec()).encode_to_vec()
}

pub fn encode_heartbeat(room_id: u64, send_packet_seq_id: u64) -> Vec<u8> {
    #[derive(Clone, PartialEq, ProstMessage)]
    struct Heartbeat {
        #[prost(uint64, tag = "1")]
        room_id: u64,
        #[prost(string, tag = "2")]
        send_packet_seq_id: String,
    }
    let body = Heartbeat {
        room_id,
        send_packet_seq_id: send_packet_seq_id.to_string(),
    };
    frame("hb", body.encode_to_vec()).encode_to_vec()
}

/// Acknowledges a message batch with the batch's `internal_ext` cursor.
pub fn encode_ack(log_id: u64, internal_ext: &str) -> Vec<u8> {
    let mut frame = frame("ack", internal_ext.as_bytes().to_vec());
    frame.log_id = log_id;
    frame.encode_to_vec()
}

// ---------------------------------------------------------------------------
// Webcast message schemas (only the fields we consume)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, ProstMessage)]
pub struct Common {
    #[prost(string, tag = "1")]
    pub method: String,
    #[prost(uint64, tag = "2")]
    pub msg_id: u64,
    #[prost(uint64, tag = "3")]
    pub room_id: u64,
    #[prost(uint64, tag = "4")]
    pub create_time: u64,
    #[prost(message, optional, tag = "8")]
    pub display_text: Option<DisplayText>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct DisplayText {
    #[prost(string, tag = "1")]
    pub key: String,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct Image {
    #[prost(string, repeated, tag = "1")]
    pub url_list: Vec<String>,
    #[prost(string, tag = "2")]
    pub uri: String,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct FollowInfo {
    #[prost(uint64, tag = "1")]
    pub following_count: u64,
    #[prost(uint64, tag = "2")]
    pub follower_count: u64,
    #[prost(uint32, tag = "3")]
    pub follow_status: u32,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct UserIdentity {
    #[prost(bool, tag = "1")]
    pub is_gifter: bool,
    #[prost(bool, tag = "2")]
    pub is_subscriber: bool,
    #[prost(bool, tag = "4")]
    pub is_follower: bool,
    #[prost(bool, tag = "5")]
    pub is_moderator: bool,
}

/// User field 12 of a gifter badge: the level is `extra.field5`.
#[derive(Clone, PartialEq, ProstMessage)]
pub struct BadgeExtra {
    #[prost(string, tag = "5")]
    pub level: String,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct ImageBadge {
    #[prost(message, optional, tag = "2")]
    pub image: Option<Image>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct UserBadge {
    #[prost(message, optional, tag = "12")]
    pub extra: Option<BadgeExtra>,
    #[prost(message, optional, tag = "23")]
    pub image_badge: Option<ImageBadge>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct User {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(string, tag = "3")]
    pub nickname: String,
    #[prost(message, optional, tag = "9")]
    pub avatar_thumb: Option<Image>,
    #[prost(message, optional, tag = "22")]
    pub follow_info: Option<FollowInfo>,
    #[prost(string, tag = "38")]
    pub unique_id: String,
    #[prost(message, repeated, tag = "64")]
    pub badges: Vec<UserBadge>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct EmoteMessage {
    #[prost(string, tag = "1")]
    pub emote_id: String,
    #[prost(message, optional, tag = "2")]
    pub image: Option<Image>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct EmoteWithIndex {
    #[prost(int32, tag = "1")]
    pub index: i32,
    #[prost(message, optional, tag = "2")]
    pub emote: Option<EmoteMessage>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct ChatMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(string, tag = "3")]
    pub comment: String,
    #[prost(message, repeated, tag = "13")]
    pub emotes: Vec<EmoteWithIndex>,
    #[prost(message, optional, tag = "18")]
    pub user_identity: Option<UserIdentity>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct GiftStruct {
    #[prost(message, optional, tag = "1")]
    pub image: Option<Image>,
    #[prost(uint32, tag = "11")]
    pub gift_type: u32,
    #[prost(uint64, tag = "12")]
    pub diamond_count: u64,
    #[prost(string, tag = "16")]
    pub name: String,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct GiftMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(uint64, tag = "2")]
    pub gift_id: u64,
    #[prost(uint64, tag = "5")]
    pub repeat_count: u64,
    #[prost(message, optional, tag = "7")]
    pub user: Option<User>,
    #[prost(bool, tag = "9")]
    pub repeat_end: bool,
    #[prost(uint64, tag = "11")]
    pub group_id: u64,
    #[prost(message, optional, tag = "15")]
    pub gift: Option<GiftStruct>,
    #[prost(message, optional, tag = "32")]
    pub user_identity: Option<UserIdentity>,
    #[prost(string, tag = "45")]
    pub to_member_id: String,
    #[prost(string, tag = "47")]
    pub to_member_nickname: String,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct LikeMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(uint64, tag = "2")]
    pub count: u64,
    #[prost(uint64, tag = "3")]
    pub total: u64,
    #[prost(message, optional, tag = "5")]
    pub user: Option<User>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct MemberMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(uint32, tag = "10")]
    pub action: u32,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct SocialMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(uint32, tag = "4")]
    pub action: u32,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct RankItem {
    #[prost(uint64, tag = "1")]
    pub coin_count: u64,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(uint32, tag = "3")]
    pub rank: u32,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct RoomUserSeqMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, repeated, tag = "2")]
    pub ranks: Vec<RankItem>,
    #[prost(uint64, tag = "3")]
    pub viewer_count: u64,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct SubNotifyMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(uint64, tag = "4")]
    pub sub_month: u64,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct EmoteChatMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(message, optional, tag = "2")]
    pub user: Option<User>,
    #[prost(message, repeated, tag = "3")]
    pub emotes: Vec<EmoteMessage>,
    #[prost(message, optional, tag = "5")]
    pub user_identity: Option<UserIdentity>,
}

#[derive(Clone, PartialEq, ProstMessage)]
pub struct ControlMessage {
    #[prost(message, optional, tag = "1")]
    pub common: Option<Common>,
    #[prost(uint32, tag = "2")]
    pub action: u32,
}

// ---------------------------------------------------------------------------
// Event decoding
// ---------------------------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct Header {
    timestamp: i64,
    msg_id: Option<String>,
    display_key: String,
}

fn header(common: Option<&Common>) -> Header {
    let Some(common) = common else {
        return Header {
            timestamp: now_ms(),
            msg_id: None,
            display_key: String::new(),
        };
    };
    // `createTime` is milliseconds on most messages, but occasionally seconds.
    let timestamp = match common.create_time {
        0 => now_ms(),
        t if t < 1_000_000_000_000 => (t as i64) * 1000,
        t => t as i64,
    };
    Header {
        timestamp,
        msg_id: (common.msg_id != 0).then(|| common.msg_id.to_string()),
        display_key: common
            .display_text
            .as_ref()
            .map(|d| d.key.clone())
            .unwrap_or_default(),
    }
}

fn image_url(image: Option<&Image>) -> String {
    image
        .and_then(|i| i.url_list.first())
        .cloned()
        .unwrap_or_default()
}

fn gifter_level(user: Option<&User>) -> Option<u32> {
    let user = user?;
    for badge in &user.badges {
        let uri = badge
            .image_badge
            .as_ref()
            .and_then(|b| b.image.as_ref())
            .map(|i| i.uri.as_str())
            .unwrap_or_default();
        if !uri.contains("grade_badge") {
            continue;
        }
        if let Some(level) = badge
            .extra
            .as_ref()
            .and_then(|e| e.level.trim().parse::<u32>().ok())
            .filter(|level| *level > 0)
        {
            return Some(level);
        }
    }
    None
}

fn decode_user(user: Option<&User>, identity: Option<&UserIdentity>) -> LiveUser {
    let Some(user) = user else {
        return LiveUser::default();
    };
    let follow_status = user
        .follow_info
        .as_ref()
        .and_then(|f| match f.follow_status {
            0 => Some(FollowStatus::None),
            1 => Some(FollowStatus::Follower),
            2 => Some(FollowStatus::Friend),
            _ => None,
        });
    LiveUser {
        user_id: user.id.to_string(),
        unique_id: user.unique_id.clone(),
        nickname: user.nickname.clone(),
        profile_picture_url: image_url(user.avatar_thumb.as_ref()),
        gifter_level: gifter_level(Some(user)),
        follower_count: user.follow_info.as_ref().map(|f| f.follower_count),
        following_count: user.follow_info.as_ref().map(|f| f.following_count),
        follow_status,
        is_gifter: identity.map(|i| i.is_gifter),
        is_subscriber: identity.map(|i| i.is_subscriber),
        is_follower: identity.map(|i| i.is_follower),
        is_moderator: identity.map(|i| i.is_moderator),
    }
}

fn decode_emote(emote: Option<&EmoteMessage>) -> Emote {
    Emote {
        emote_id: emote.map(|e| e.emote_id.clone()).unwrap_or_default(),
        image_url: image_url(emote.and_then(|e| e.image.as_ref())),
    }
}

/// Maps a raw message onto a normalized event. Unknown messages are `None`.
pub fn decode_live_event(method: &str, payload: &[u8]) -> Option<LiveEvent> {
    Some(match method {
        "WebcastChatMessage" => {
            let m = ChatMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            LiveEvent::Chat {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), m.user_identity.as_ref()),
                comment: m.comment,
                emotes: m
                    .emotes
                    .iter()
                    .map(|e| decode_emote(e.emote.as_ref()))
                    .collect(),
            }
        }
        "WebcastGiftMessage" => {
            let m = GiftMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            let gift = m.gift.as_ref();
            let diamond_count = gift.map(|g| g.diamond_count).unwrap_or(0);
            let repeat_count = m.repeat_count.max(1);
            let streaking = gift.map(|g| g.gift_type == 1).unwrap_or(false) && !m.repeat_end;
            LiveEvent::Gift {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), m.user_identity.as_ref()),
                gift_id: m.gift_id,
                gift_name: gift.map(|g| g.name.clone()).unwrap_or_default(),
                gift_image_url: image_url(gift.and_then(|g| g.image.as_ref())),
                diamond_count,
                repeat_count,
                streaking,
                total_diamonds: diamond_count * repeat_count,
                group_id: m.group_id.to_string(),
                receiver: (!m.to_member_id.is_empty() && m.to_member_id != "0").then(|| {
                    GiftReceiver {
                        user_id: m.to_member_id.clone(),
                        nickname: m.to_member_nickname.clone(),
                    }
                }),
            }
        }
        "WebcastLikeMessage" => {
            let m = LikeMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            LiveEvent::Like {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), None),
                like_count: m.count,
                total_like_count: m.total,
            }
        }
        "WebcastMemberMessage" => {
            let m = MemberMessage::decode(payload).ok()?;
            // Action 1 is "joined"; the others are moderation actions.
            if m.action != 1 {
                return None;
            }
            let h = header(m.common.as_ref());
            LiveEvent::Join {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), None),
            }
        }
        "WebcastSocialMessage" => {
            let m = SocialMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            let user = decode_user(m.user.as_ref(), None);
            if h.display_key.contains("follow") || (h.display_key.is_empty() && m.action == 1) {
                LiveEvent::Follow {
                    timestamp: h.timestamp,
                    msg_id: h.msg_id,
                    user,
                }
            } else if h.display_key.contains("share") || (h.display_key.is_empty() && m.action == 3)
            {
                LiveEvent::Share {
                    timestamp: h.timestamp,
                    msg_id: h.msg_id,
                    user,
                }
            } else {
                return None;
            }
        }
        "WebcastRoomUserSeqMessage" => {
            let m = RoomUserSeqMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            LiveEvent::Viewers {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                viewer_count: m.viewer_count,
                top_viewers: m
                    .ranks
                    .iter()
                    .map(|r| TopViewer {
                        user: decode_user(r.user.as_ref(), None),
                        coin_count: r.coin_count,
                        rank: r.rank,
                    })
                    .collect(),
            }
        }
        "WebcastSubNotifyMessage" => {
            let m = SubNotifyMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            LiveEvent::Subscribe {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), None),
                sub_month: m.sub_month,
            }
        }
        "WebcastEmoteChatMessage" => {
            let m = EmoteChatMessage::decode(payload).ok()?;
            let h = header(m.common.as_ref());
            // Emote-only comments are chat messages with no plain text.
            let emotes: Vec<Emote> = m.emotes.iter().map(|e| decode_emote(Some(e))).collect();
            if emotes.is_empty() {
                return None;
            }
            LiveEvent::Chat {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                user: decode_user(m.user.as_ref(), m.user_identity.as_ref()),
                comment: String::new(),
                emotes,
            }
        }
        "WebcastControlMessage" => {
            let m = ControlMessage::decode(payload).ok()?;
            let reason = match m.action {
                3 => "ended",
                4 => "suspended",
                _ => return None,
            };
            let h = header(m.common.as_ref());
            LiveEvent::StreamEnd {
                timestamp: h.timestamp,
                msg_id: h.msg_id,
                reason: reason.into(),
            }
        }
        _ => return None,
    })
}

/// Decodes a raw batch message into a normalized event.
pub fn decode_message(raw: &WebcastMessage) -> Option<LiveEvent> {
    let event = decode_live_event(&raw.method, &raw.payload)?;
    Some(with_envelope_id(event, &raw.msg_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_room_frame_roundtrips() {
        let bytes = encode_enter_room(123456789, "", 42);
        let frame = PushFrame::decode(bytes.as_slice()).unwrap();
        assert_eq!(frame.payload_type, "im_enter_room");
        assert_eq!(frame.payload_encoding, "pb");

        #[derive(Clone, PartialEq, ProstMessage)]
        struct EnterRoom {
            #[prost(uint64, tag = "1")]
            room_id: u64,
            #[prost(uint64, tag = "4")]
            live_id: u64,
            #[prost(string, tag = "5")]
            identity: String,
            #[prost(string, tag = "6")]
            cursor: String,
            #[prost(uint64, tag = "8")]
            enter_unique_id: u64,
        }
        let body = EnterRoom::decode(frame.payload.as_slice()).unwrap();
        assert_eq!(body.room_id, 123456789);
        assert_eq!(body.live_id, LIVE_ID);
        assert_eq!(body.identity, "audience");
        assert_eq!(body.enter_unique_id, 42);
    }

    #[test]
    fn numeric_service_and_method_decode() {
        let result = FetchResult {
            cursor: "token".into(),
            ..Default::default()
        };
        let mut push = frame("msg", result.encode_to_vec());
        push.service = 8888;
        push.method = 8;
        let decoded = decode_socket_frame(&push.encode_to_vec()).unwrap();
        assert_eq!(decoded.frame.service, 8888);
        assert_eq!(decoded.frame.method, 8);
        assert_eq!(decoded.result.unwrap().cursor, "token");
    }

    #[test]
    fn heartbeat_and_ack_frames() {
        let hb = PushFrame::decode(encode_heartbeat(7, 3).as_slice()).unwrap();
        assert_eq!(hb.payload_type, "hb");

        let ack = PushFrame::decode(encode_ack(99, "cursor").as_slice()).unwrap();
        assert_eq!(ack.payload_type, "ack");
        assert_eq!(ack.log_id, 99);
        assert_eq!(ack.payload, b"cursor");
    }

    #[test]
    fn chat_message_decodes() {
        let message = ChatMessage {
            common: Some(Common {
                method: "WebcastChatMessage".into(),
                msg_id: 42,
                create_time: 1_700_000_000_000,
                ..Default::default()
            }),
            user: Some(User {
                id: 7,
                nickname: "Bo".into(),
                unique_id: "bo".into(),
                ..Default::default()
            }),
            comment: "hello".into(),
            ..Default::default()
        };
        let raw = WebcastMessage {
            method: "WebcastChatMessage".into(),
            payload: message.encode_to_vec(),
            msg_id: 42,
            is_history: false,
        };
        let event = decode_message(&raw).unwrap();
        match event {
            LiveEvent::Chat {
                comment,
                user,
                msg_id,
                ..
            } => {
                assert_eq!(comment, "hello");
                assert_eq!(user.nickname, "Bo");
                assert_eq!(msg_id.as_deref(), Some("42"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn gift_streak_flags() {
        let message = GiftMessage {
            common: Some(Common {
                msg_id: 5,
                create_time: 1_700_000_000,
                ..Default::default()
            }),
            gift_id: 1,
            repeat_count: 3,
            gift: Some(GiftStruct {
                name: "Rose".into(),
                diamond_count: 2,
                gift_type: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let event = decode_live_event("WebcastGiftMessage", &message.encode_to_vec()).unwrap();
        match event {
            LiveEvent::Gift {
                streaking,
                total_diamonds,
                ..
            } => {
                assert!(streaking);
                assert_eq!(total_diamonds, 6);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
