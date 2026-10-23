//! HTTPS calls to TikTok plus the EulerStream signed-WebSocket handshake.

use std::sync::{Arc, Mutex};

use prost::Message;
use reqwest::cookie::CookieStore;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, REFERER, SET_COOKIE,
};
use reqwest::Url;
use serde::Deserialize;

use crate::error::{LiveError, Result};
use crate::events::{GiftInfo, RoomOwner, RoomStatus};
use crate::proto::FetchResult;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";

/// Accepts `name`, `@name`, or a TikTok profile/live URL and returns the bare
/// lowercase handle.
pub fn normalize_username(input: &str) -> String {
    let trimmed = input.trim();
    let from_url = find_tiktok_handle(trimmed).unwrap_or(trimmed);
    from_url.trim_start_matches('@').to_lowercase()
}

fn find_tiktok_handle(input: &str) -> Option<&str> {
    let lower = input.to_ascii_lowercase();
    let at = lower.find("tiktok.com/@")?;
    let rest = &input[at + "tiktok.com/@".len()..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    (!rest[..end].is_empty()).then_some(&rest[..end])
}

/// A short-lived signed WebSocket handshake for one room.
pub struct SignedSocket {
    pub push_server: String,
    pub cookie: String,
    pub initial: FetchResult,
}

/// A signed-out TikTok web client: page cookies, room lookup and gift catalog.
#[derive(Clone)]
pub struct TikTokWeb {
    client: reqwest::Client,
    jar: Arc<reqwest::cookie::Jar>,
    user_agent: String,
    cached_ttwid: Arc<Mutex<Option<String>>>,
}

impl TikTokWeb {
    pub fn new(user_agent: Option<String>) -> Result<Self> {
        let user_agent = user_agent.unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());
        let jar = Arc::new(reqwest::cookie::Jar::default());

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
        headers.insert(REFERER, HeaderValue::from_static("https://www.tiktok.com/"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );

        let client = reqwest::Client::builder()
            .user_agent(&user_agent)
            .cookie_provider(jar.clone())
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        Ok(Self {
            client,
            jar,
            user_agent,
            cached_ttwid: Arc::new(Mutex::new(None)),
        })
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// An anonymous TikTok device cookie, used by the unsigned fallback path.
    /// Cached unless `refresh`.
    pub async fn ttwid(&self, refresh: bool) -> Result<String> {
        if !refresh {
            if let Some(cached) = self.cached_ttwid.lock().expect("ttwid cache").as_ref() {
                return Ok(cached.clone());
            }
        }

        let _res = self
            .client
            .get("https://www.tiktok.com/live")
            .send()
            .await?;
        let url = Url::parse("https://www.tiktok.com/live")?;
        let cookie = self
            .jar
            .cookies(&url)
            .and_then(|value| cookie_value(&value, "ttwid"));

        let ttwid = match cookie {
            Some(cookie) => cookie,
            None => self.register_ttwid().await?,
        };

        *self.cached_ttwid.lock().expect("ttwid cache") = Some(ttwid.clone());
        Ok(ttwid)
    }

    /// TikTok's webcast socket now requires a request signature. EulerStream
    /// returns the signed route parameters, the initial event batch and the
    /// anonymous cookie accepted by the signed socket.
    pub async fn signed_socket(&self, room_id: &str) -> Result<SignedSocket> {
        let base = std::env::var("SIGN_API_URL")
            .unwrap_or_else(|_| "https://api.eulerstream.com".to_string());
        let endpoint = format!(
            "{}/webcast/rooms/{room_id}/connect",
            base.trim_end_matches('/')
        );
        let mut request = self
            .client
            .get(endpoint)
            .query(&[
                ("client", "ttlive-rs"),
                ("user_agent", self.user_agent.as_str()),
                ("client_enter", "true"),
                ("platform", "web"),
            ])
            .header(ACCEPT, "application/protobuf,application/json")
            .header("User-Agent", "tiktok-live-connector/2.5.0 win32");
        if let Ok(api_key) = std::env::var("SIGN_API_KEY") {
            if !api_key.is_empty() {
                request = request.header("x-api-key", api_key);
            }
        }
        let res = request.send().await?;

        let status = res.status().as_u16();
        let cookie = res
            .headers()
            .get("x-set-tt-cookie")
            .and_then(|value| value.to_str().ok())
            .map(decode_percent);
        let bytes = res.bytes().await?;

        if status != 200 {
            let detail = String::from_utf8_lossy(&bytes);
            return Err(if status == 429 {
                LiveError::RateLimited { status }
            } else {
                LiveError::Signing(format!("HTTP {status}: {detail}"))
            });
        }

        let Some(cookie) = cookie.filter(|cookie| !cookie.is_empty()) else {
            return Err(LiveError::Signing("response had no x-set-tt-cookie".into()));
        };
        let initial = FetchResult::decode(bytes.as_ref())
            .map_err(|e| LiveError::Signing(format!("invalid protobuf response: {e}")))?;
        if initial.push_server.is_empty() {
            return Err(LiveError::Signing("response had no push server".into()));
        }

        Ok(SignedSocket {
            push_server: initial.push_server.clone(),
            cookie,
            initial,
        })
    }

    /// TikTok's page can be replaced by a WAF challenge that never sets
    /// `ttwid`. The union registration endpoint still issues one directly.
    async fn register_ttwid(&self) -> Result<String> {
        let body = serde_json::json!({
            "region": "cn",
            "aid": 1768,
            "needFid": false,
            "service": "www.tiktok.com",
            "migrate_info": { "ticket": "", "source": "node" },
            "cbUrlProtocol": "https",
            "union": true
        });
        let res = self
            .client
            .post("https://ttwid.bytedance.com/ttwid/union/register/")
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;
        let status = res.status().as_u16();
        let cookie = res
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .find_map(|header| cookie_value(header, "ttwid"));

        let Some(ttwid) = cookie else {
            return Err(if status == 429 {
                LiveError::RateLimited { status }
            } else {
                LiveError::TtwidMissing { status }
            });
        };
        Ok(ttwid)
    }

    /// Looks up the user's current room and whether it is live.
    pub async fn room_status(&self, username: &str) -> Result<RoomStatus> {
        let unique_id = normalize_username(username);
        let url = format!(
            "https://www.tiktok.com/api-live/user/room/?aid=1988&app_name=tiktok_web&device_platform=web_pc&uniqueId={unique_id}&sourceType=54"
        );
        let res = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .send()
            .await?;
        let status = res.status().as_u16();
        let text = res.text().await?;

        if !(200..300).contains(&status) {
            return Err(LiveError::RoomLookup(format!("HTTP {status}")));
        }
        if text.is_empty() {
            return Err(LiveError::RoomLookup(
                "empty response (likely rate limited)".into(),
            ));
        }

        let body: RoomApiResponse = serde_json::from_str(&text)
            .map_err(|e| LiveError::RoomLookup(format!("invalid JSON: {e}")))?;

        if body.status_code == Some(19881007) {
            return Err(LiveError::UserNotFound(unique_id));
        }
        if let Some(code) = body.status_code.filter(|c| *c != 0) {
            return Err(LiveError::RoomLookup(
                body.message
                    .clone()
                    .unwrap_or_else(|| format!("status {code}")),
            ));
        }

        let Some(data) = body.data else {
            return Err(LiveError::RoomLookup("no user data".into()));
        };
        let Some(user) = data.user else {
            return Err(LiveError::RoomLookup("no user data".into()));
        };

        let live_room = data.live_room.unwrap_or_default();
        let stats = data.stats.unwrap_or_default();
        let room_id = user.room_id.unwrap_or_default();
        let is_live = live_room.status == Some(2) && !room_id.is_empty();
        let start_time = live_room.start_time.unwrap_or(0);

        Ok(RoomStatus {
            room_id: room_id.clone(),
            is_live,
            title: live_room.title.unwrap_or_default(),
            viewer_count: live_room
                .live_room_stats
                .and_then(|s| s.user_count)
                .unwrap_or(0),
            owner: RoomOwner {
                user_id: user.id.unwrap_or_default(),
                unique_id: user.unique_id.unwrap_or(unique_id),
                nickname: user.nickname.unwrap_or_default(),
                profile_picture_url: user.avatar_thumb.unwrap_or_default(),
                follower_count: stats.follower_count.unwrap_or(0),
            },
            stream_started_at: (start_time > 0).then_some(start_time as i64 * 1000),
            cover_url: live_room.cover_url,
            hls_url: live_room
                .stream_data
                .and_then(|s| s.pull_data)
                .and_then(|p| p.stream_data)
                .and_then(|json| pick_hls_url(&json)),
        })
    }

    /// The public gift catalog, sorted by price.
    pub async fn gifts(&self) -> Result<Vec<GiftInfo>> {
        let res = self
            .client
            .get("https://webcast.tiktok.com/webcast/gift/list/?aid=1988&app_name=tiktok_web&device_platform=web_pc&browser_language=en-US")
            .header(ACCEPT, "application/json")
            .send()
            .await?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(LiveError::RoomLookup(format!("gift list HTTP {status}")));
        }
        let body: GiftApiResponse = res
            .json()
            .await
            .map_err(|e| LiveError::RoomLookup(format!("gift list JSON: {e}")))?;
        if let Some(code) = body.status_code.filter(|c| *c != 0) {
            return Err(LiveError::RoomLookup(format!("gift list status {code}")));
        }
        let mut gifts: Vec<GiftInfo> = body
            .data
            .and_then(|d| d.gifts)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .filter_map(|gift| {
                Some(GiftInfo {
                    gift_id: gift.id?,
                    name: gift.name?,
                    diamond_count: gift.diamond_count.unwrap_or(0),
                    image_url: gift
                        .image
                        .and_then(|i| i.url_list)
                        .and_then(|urls| urls.into_iter().next())
                        .unwrap_or_default(),
                    streakable: gift.r#type == Some(1),
                })
            })
            .collect();
        gifts.sort_by(|a, b| {
            a.diamond_count
                .cmp(&b.diamond_count)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(gifts)
    }
}

fn cookie_value(header: &HeaderValue, name: &str) -> Option<String> {
    let raw = header.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn decode_percent(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(hex) = value.get(i + 1..i + 3) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Smaller first: the URL is for previews, not re-streaming.
const HLS_QUALITIES: [&str; 5] = ["sd", "ld", "hd", "uhd", "origin"];

/// The HLS URL from the room's `stream_data` JSON, preferring a small quality.
pub fn pick_hls_url(stream_data: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct StreamData {
        data: Option<std::collections::HashMap<String, Quality>>,
    }
    #[derive(Deserialize)]
    struct Quality {
        main: Option<Main>,
    }
    #[derive(Deserialize)]
    struct Main {
        hls: Option<String>,
    }

    let parsed: StreamData = serde_json::from_str(stream_data).ok()?;
    let qualities = parsed.data?;
    let preferred = HLS_QUALITIES
        .iter()
        .copied()
        .chain(qualities.keys().map(|k| k.as_str()).filter(|k| *k != "ao"));
    for key in preferred {
        if let Some(url) = qualities
            .get(key)
            .and_then(|q| q.main.as_ref())
            .and_then(|m| m.hls.as_deref())
        {
            if url.starts_with("https://") {
                return Some(url.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct RoomApiResponse {
    #[serde(rename = "statusCode")]
    status_code: Option<i64>,
    message: Option<String>,
    data: Option<RoomData>,
}

#[derive(Debug, Deserialize)]
struct RoomData {
    user: Option<ApiUser>,
    stats: Option<ApiStats>,
    #[serde(rename = "liveRoom")]
    live_room: Option<ApiLiveRoom>,
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    id: Option<String>,
    #[serde(rename = "uniqueId")]
    unique_id: Option<String>,
    nickname: Option<String>,
    #[serde(rename = "avatarThumb")]
    avatar_thumb: Option<String>,
    #[serde(rename = "roomId")]
    room_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ApiStats {
    #[serde(rename = "followerCount")]
    follower_count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ApiLiveRoom {
    status: Option<u32>,
    title: Option<String>,
    #[serde(rename = "liveRoomStats")]
    live_room_stats: Option<ApiLiveRoomStats>,
    #[serde(rename = "startTime")]
    start_time: Option<u64>,
    #[serde(rename = "coverUrl")]
    cover_url: Option<String>,
    #[serde(rename = "streamData")]
    stream_data: Option<ApiStreamData>,
}

#[derive(Debug, Deserialize)]
struct ApiLiveRoomStats {
    #[serde(rename = "userCount")]
    user_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApiStreamData {
    #[serde(rename = "pull_data")]
    pull_data: Option<ApiPullData>,
}

#[derive(Debug, Deserialize)]
struct ApiPullData {
    stream_data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GiftApiResponse {
    status_code: Option<i64>,
    data: Option<GiftData>,
}

#[derive(Debug, Deserialize)]
struct GiftData {
    gifts: Option<Vec<Option<ApiGift>>>,
}

#[derive(Debug, Deserialize)]
struct ApiGift {
    id: Option<u64>,
    name: Option<String>,
    diamond_count: Option<u64>,
    #[serde(rename = "type")]
    r#type: Option<u32>,
    image: Option<ApiImage>,
}

#[derive(Debug, Deserialize)]
struct ApiImage {
    url_list: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_handles_and_urls() {
        assert_eq!(normalize_username(" @Some.One "), "some.one");
        assert_eq!(
            normalize_username("https://www.tiktok.com/@zaynscape/live"),
            "zaynscape"
        );
    }

    #[test]
    fn picks_hls_quality() {
        let json = r#"{"data":{"hd":{"main":{"hls":"https://hd"}},"sd":{"main":{"hls":"https://sd"}},"ao":{"main":{"hls":"https://audio"}}}}"#;
        assert_eq!(pick_hls_url(json).as_deref(), Some("https://sd"));
    }
}
