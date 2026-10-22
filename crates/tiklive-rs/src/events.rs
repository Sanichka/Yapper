//! Normalized, JSON-serializable LIVE events.
//!
//! Mirrors `@tiklive/contracts`: the flat `type`-tagged shapes the app and
//! overlays consume, decoupled from TikTok's raw webcast protobuf.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveUser {
    pub user_id: String,
    /// The `@handle`, without the leading `@`.
    pub unique_id: String,
    pub nickname: String,
    pub profile_picture_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_moderator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_subscriber: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_follower: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_gifter: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gifter_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follower_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_status: Option<FollowStatus>,
}

impl LiveUser {
    /// Best display name: nickname, then handle, then user id.
    pub fn display_name(&self) -> &str {
        if !self.nickname.is_empty() {
            &self.nickname
        } else if !self.unique_id.is_empty() {
            &self.unique_id
        } else {
            &self.user_id
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FollowStatus {
    None,
    Follower,
    Friend,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Emote {
    pub emote_id: String,
    pub image_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TopViewer {
    pub user: LiveUser,
    pub coin_count: u64,
    pub rank: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GiftReceiver {
    pub user_id: String,
    pub nickname: String,
}

/// Every event carries a `type` discriminator and a unix-ms `timestamp`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LiveEvent {
    Chat {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
        comment: String,
        emotes: Vec<Emote>,
    },
    Gift {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
        gift_id: u64,
        gift_name: String,
        gift_image_url: String,
        /// Coin value of a single gift.
        diamond_count: u64,
        /// Running count within the current combo/streak.
        repeat_count: u64,
        /// `true` while the combo is still going; only the final event is `false`.
        streaking: bool,
        total_diamonds: u64,
        group_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        receiver: Option<GiftReceiver>,
    },
    Like {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
        like_count: u64,
        total_like_count: u64,
    },
    /// A viewer joined the room (`MemberEvent`).
    Join {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
    },
    Follow {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
    },
    Share {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
    },
    Viewers {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        viewer_count: u64,
        top_viewers: Vec<TopViewer>,
    },
    Subscribe {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        user: LiveUser,
        sub_month: u64,
    },
    StreamEnd {
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        reason: String,
    },
}

impl LiveEvent {
    pub fn timestamp(&self) -> i64 {
        match self {
            LiveEvent::Chat { timestamp, .. }
            | LiveEvent::Gift { timestamp, .. }
            | LiveEvent::Like { timestamp, .. }
            | LiveEvent::Join { timestamp, .. }
            | LiveEvent::Follow { timestamp, .. }
            | LiveEvent::Share { timestamp, .. }
            | LiveEvent::Viewers { timestamp, .. }
            | LiveEvent::Subscribe { timestamp, .. }
            | LiveEvent::StreamEnd { timestamp, .. } => *timestamp,
        }
    }

    pub fn msg_id(&self) -> Option<&str> {
        match self {
            LiveEvent::Chat { msg_id, .. }
            | LiveEvent::Gift { msg_id, .. }
            | LiveEvent::Like { msg_id, .. }
            | LiveEvent::Join { msg_id, .. }
            | LiveEvent::Follow { msg_id, .. }
            | LiveEvent::Share { msg_id, .. }
            | LiveEvent::Viewers { msg_id, .. }
            | LiveEvent::Subscribe { msg_id, .. }
            | LiveEvent::StreamEnd { msg_id, .. } => msg_id.as_deref(),
        }
    }

    pub fn user(&self) -> Option<&LiveUser> {
        match self {
            LiveEvent::Chat { user, .. }
            | LiveEvent::Gift { user, .. }
            | LiveEvent::Like { user, .. }
            | LiveEvent::Join { user, .. }
            | LiveEvent::Follow { user, .. }
            | LiveEvent::Share { user, .. }
            | LiveEvent::Subscribe { user, .. } => Some(user),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomOwner {
    pub user_id: String,
    pub unique_id: String,
    pub nickname: String,
    pub profile_picture_url: String,
    pub follower_count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomStatus {
    pub room_id: String,
    pub is_live: bool,
    pub title: String,
    pub viewer_count: u64,
    pub owner: RoomOwner,
    /// Unix ms when TikTok says the stream started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hls_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GiftInfo {
    pub gift_id: u64,
    pub name: String,
    pub diamond_count: u64,
    pub image_url: String,
    pub streakable: bool,
}

/// Fills in the envelope `msgId` when the message body has none.
pub(crate) fn with_envelope_id(mut event: LiveEvent, envelope_msg_id: &str) -> LiveEvent {
    if envelope_msg_id.is_empty() || envelope_msg_id == "0" {
        return event;
    }
    let slot = match &mut event {
        LiveEvent::Chat { msg_id, .. }
        | LiveEvent::Gift { msg_id, .. }
        | LiveEvent::Like { msg_id, .. }
        | LiveEvent::Join { msg_id, .. }
        | LiveEvent::Follow { msg_id, .. }
        | LiveEvent::Share { msg_id, .. }
        | LiveEvent::Viewers { msg_id, .. }
        | LiveEvent::Subscribe { msg_id, .. }
        | LiveEvent::StreamEnd { msg_id, .. } => msg_id,
    };
    if slot.is_none() {
        *slot = Some(envelope_msg_id.to_string());
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_serializes_with_type_tag() {
        let event = LiveEvent::Chat {
            timestamp: 1,
            msg_id: Some("42".into()),
            user: LiveUser {
                nickname: "Bo".into(),
                ..Default::default()
            },
            comment: "hi".into(),
            emotes: vec![],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "chat");
        assert_eq!(json["msgId"], "42");
        assert_eq!(json["user"]["nickname"], "Bo");
    }
}
