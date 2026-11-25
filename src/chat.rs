//! Live chat through the Data API: the channel's broadcasts (liveBroadcasts), a live video's chat
//! (videos.liveStreamingDetails.activeLiveChatId), its messages (liveChatMessages.list, polled with the page token and
//! the wait YouTube asks for), and the moderator's actions: sending, deleting and timing out (liveChatMessages.insert
//! and .delete, liveChatBans.insert). Those last three need a chat the channel owns or moderates, and the `youtube` or
//! `youtube.force-ssl` scope.
//!
//! What's posted to a Pointman channel is other people's words: [`defang`] keeps a chat message from @mentioning anyone on the
//! board, or from being read as a slash command.

use anyhow::bail;
use serde_json::{json, Value};

use crate::google::{Api, Failure};
use crate::params::timestamp;
use crate::v1::{Broadcast, ChatMessage};

/// The scopes that let a sign-in send, delete and time out in a live chat (Google's liveChatMessages.insert and
/// .delete, liveChatBans.insert).
pub const MODERATE: [&str; 2] =
    ["https://www.googleapis.com/auth/youtube", "https://www.googleapis.com/auth/youtube.force-ssl"];

/// The most messages one liveChatMessages.list call gives.
pub const MOST: &str = "2000";

/// Whether these scopes (space-separated, as Google gives them) let a sign-in moderate a chat.
pub fn can_moderate(scopes: &str) -> bool {
    scopes.split_whitespace().any(|s| MODERATE.contains(&s))
}

/// A Pointman channel param as the board names it: `ch_123` (or a bare `123`).
pub fn channel_id(given: &str) -> String {
    let n = given.trim().trim_start_matches("ch_");
    format!("ch_{n}")
}

/// An agent's handle with its @, or nothing.
pub fn mention(given: &str) -> String {
    match given.trim().trim_start_matches('@') {
        "" => String::new(),
        h => format!("@{h}"),
    }
}

/// A chat's words made safe to post: an @ can't mention anyone on the board (a zero-width space after it), a line
/// can't start a slash command, and newlines become spaces, so each message stays on its own row.
pub fn defang(text: &str) -> String {
    let one_line: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    let safe = one_line.replace('@', "@\u{200B}");
    match safe.strip_prefix('/') {
        Some(rest) => format!("\u{200B}/{rest}"),
        None => safe,
    }
}

/// A live video whose chat isn't open yet: a scheduled stream that hasn't started, or one with its chat still off.
/// Worth waiting for, unlike a video that isn't live at all or has ended.
#[derive(Debug)]
pub struct NotOpen(pub String);

impl std::fmt::Display for NotOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotOpen {}

/// Whether an error is a chat that isn't open yet ([`NotOpen`]).
pub fn not_open(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotOpen>().is_some()
}

/// A live video's chat: its id and the video's title, or an error saying why there's none ([`NotOpen`] when it may
/// still open).
pub fn of_video(api: &Api, video: &str) -> anyhow::Result<(String, String)> {
    let got = api.get("videos", &[("part", "snippet,liveStreamingDetails"), ("id", video)])?;
    let Some(v) = got["items"].as_array().and_then(|i| i.first()) else {
        bail!("YouTube has no video {video} that this sign-in can see");
    };
    let title = v["snippet"]["title"].as_str().unwrap_or_default().to_string();
    let live = &v["liveStreamingDetails"];
    match live["activeLiveChatId"].as_str().filter(|c| !c.is_empty()) {
        Some(chat) => Ok((chat.to_string(), title)),
        None if live.is_null() => bail!("\"{title}\" ({video}) isn't a live stream, so it has no live chat"),
        None if live["actualEndTime"].is_string() => bail!("\"{title}\" ({video}) has ended: its live chat is closed"),
        None => Err(NotOpen(format!(
            "\"{title}\" ({video}) has no live chat open yet: the stream hasn't started, or its chat is off"
        ))
        .into()),
    }
}

/// The chat to use: `chat_id` as given, else `video`'s.
pub fn resolve(api: &Api, video: &str, chat_id: &str) -> anyhow::Result<String> {
    match (chat_id.trim(), video.trim()) {
        ("", "") => bail!("give the live video (video) or its chat (chat_id)"),
        ("", v) => Ok(of_video(api, v)?.0),
        (c, _) => Ok(c.to_string()),
    }
}

/// One message as liveChatMessages gives it.
pub fn message(item: &Value) -> ChatMessage {
    let (s, a) = (&item["snippet"], &item["authorDetails"]);
    let str = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let text = s["displayMessage"]
        .as_str()
        .filter(|t| !t.is_empty())
        .or_else(|| s["textMessageDetails"]["messageText"].as_str())
        .unwrap_or_default();
    ChatMessage {
        id: str(&item["id"]),
        author: str(&a["displayName"]),
        author_channel_id: a["channelId"].as_str().or(s["authorChannelId"].as_str()).unwrap_or_default().to_string(),
        text: text.to_string(),
        at: s["publishedAt"].as_str().and_then(timestamp).into(),
        is_owner: a["isChatOwner"].as_bool().unwrap_or(false),
        is_moderator: a["isChatModerator"].as_bool().unwrap_or(false),
        kind: str(&s["type"]),
        ..Default::default()
    }
}

/// One page of a chat.
#[derive(Debug, Default)]
pub struct Page {
    pub messages: Vec<ChatMessage>,
    pub next: String,
    pub poll_ms: i32,
    /// When the chat went offline, as YouTube says it (RFC 3339), if it has.
    pub offline_at: Option<String>,
}

/// The chat's messages after `page_token` (the latest, without one), oldest first. An ended chat is
/// [`Ended`](Failure::Said) `liveChatEnded`, as YouTube says it.
pub fn page(api: &Api, chat: &str, page_token: &str, max: &str) -> anyhow::Result<Page> {
    let mut q = vec![("liveChatId", chat), ("part", "id,snippet,authorDetails"), ("maxResults", max)];
    if !page_token.is_empty() {
        q.push(("pageToken", page_token));
    }
    let got = api.get("liveChat/messages", &q)?;
    Ok(Page {
        messages: got["items"].as_array().into_iter().flatten().map(message).collect(),
        next: got["nextPageToken"].as_str().unwrap_or_default().to_string(),
        poll_ms: got["pollingIntervalMillis"].as_i64().unwrap_or(0).clamp(0, i32::MAX as i64) as i32,
        offline_at: got["offlineAt"].as_str().map(String::from),
    })
}

/// Whether an error is YouTube saying the chat is over (or gone).
pub fn ended(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<Failure>(),
        Some(Failure::Said { reason, .. }) if reason == "liveChatEnded" || reason == "liveChatNotFound"
    )
}

/// Whether an error is YouTube saying the channel can't live stream (it has no broadcasts to list).
pub fn not_enabled(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<Failure>(), Some(Failure::Said { reason, .. }) if reason == "liveStreamingNotEnabled")
}

/// The channel's broadcasts with `status` (YouTube's broadcastStatus: active or upcoming).
pub fn broadcasts(api: &Api, status: &str) -> anyhow::Result<Vec<Broadcast>> {
    let got = api.get(
        "liveBroadcasts",
        &[("part", "id,snippet,status"), ("broadcastStatus", status), ("broadcastType", "all"), ("maxResults", "50")],
    )?;
    let word = if status == "active" { "live" } else { "upcoming" };
    let mut out: Vec<Broadcast> = got["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let s = &b["snippet"];
            let id = b["id"].as_str().unwrap_or_default();
            let start = if word == "live" { &s["actualStartTime"] } else { &s["scheduledStartTime"] };
            let start = start.as_str().or(s["scheduledStartTime"].as_str());
            Broadcast {
                video: id.to_string(),
                title: s["title"].as_str().unwrap_or_default().to_string(),
                status: word.to_string(),
                chat_id: s["liveChatId"].as_str().unwrap_or_default().to_string(),
                starts_at: start.and_then(timestamp).into(),
                url: crate::channel::url(id),
                ..Default::default()
            }
        })
        .collect();
    out.sort_by_key(|b| b.starts_at.as_option().map(|t| t.seconds).unwrap_or(i64::MAX));
    Ok(out)
}

/// Sends `text` in the chat as the channel: the new message's id.
pub fn send(api: &Api, chat: &str, text: &str) -> anyhow::Result<String> {
    let body = json!({"snippet": {"liveChatId": chat, "type": "textMessageEvent",
                                  "textMessageDetails": {"messageText": text}}});
    let made = api.send_json("POST", "liveChat/messages", &[("part", "snippet")], &body)?;
    Ok(made["id"].as_str().unwrap_or_default().to_string())
}

/// Deletes a message.
pub fn remove(api: &Api, message_id: &str) -> anyhow::Result<()> {
    api.delete("liveChat/messages", &[("id", message_id)])
}

/// Keeps a channel out of the chat for `seconds`: the ban's id.
pub fn timeout(api: &Api, chat: &str, channel: &str, seconds: i32) -> anyhow::Result<String> {
    let body = json!({"snippet": {"liveChatId": chat, "type": "temporary", "banDurationSeconds": seconds,
                                  "bannedUserDetails": {"channelId": channel}}});
    let made = api.send_json("POST", "liveChat/bans", &[("part", "snippet")], &body)?;
    Ok(made["id"].as_str().unwrap_or_default().to_string())
}

/// `HH:MM:SS UTC` of a message's time.
pub fn clock(m: &ChatMessage) -> String {
    m.at.as_option()
        .map(|t| {
            let s = t.seconds.rem_euclid(86_400);
            format!("{:02}:{:02}:{:02} UTC", s / 3600, s / 60 % 60, s % 60)
        })
        .unwrap_or_else(|| "--:--:-- UTC".into())
}

/// Who wrote it, with owner or moderator marked.
pub fn who(m: &ChatMessage) -> String {
    let mut name = defang(&m.author);
    if m.is_owner {
        name.push_str(" (owner)");
    } else if m.is_moderator {
        name.push_str(" (mod)");
    }
    name
}

/// One deletion's line in the log: the author, their words, the time and the reason.
pub fn removed_line(m: &ChatMessage, reason: &str) -> String {
    format!(
        "🗑 Removed from YouTube chat: {} ({}) at {}: “{}”. Reason: {}",
        who(m),
        m.author_channel_id,
        clock(m),
        defang(&m.text),
        defang(reason)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_words_never_mention_or_command() {
        assert_eq!(defang("hi @james look"), "hi @\u{200B}james look");
        assert_eq!(defang("/despawn loupe"), "\u{200B}/despawn loupe");
        assert_eq!(defang("one\ntwo"), "one two");
        assert!(!defang("@loupe").contains("@l"));
    }

    #[test]
    fn a_message_reads_as_youtube_gives_it() {
        let m = message(&json!({
            "id": "LCC.abc",
            "snippet": {"type": "textMessageEvent", "publishedAt": "2026-11-18T14:03:07.5Z",
                        "authorChannelId": "UCx", "displayMessage": "hello",
                        "textMessageDetails": {"messageText": "hello"}},
            "authorDetails": {"displayName": "Ana", "channelId": "UCx", "isChatOwner": false, "isChatModerator": true}
        }));
        assert_eq!((m.id.as_str(), m.author.as_str(), m.text.as_str()), ("LCC.abc", "Ana", "hello"));
        assert!(m.is_moderator && !m.is_owner);
        assert_eq!(clock(&m), "14:03:07 UTC");
        assert_eq!(who(&m), "Ana (mod)");
    }

    #[test]
    fn moderating_takes_youtube_or_force_ssl() {
        assert!(can_moderate("https://www.googleapis.com/auth/youtube"));
        assert!(can_moderate("openid https://www.googleapis.com/auth/youtube.force-ssl"));
        assert!(!can_moderate("https://www.googleapis.com/auth/youtube.readonly"));
        assert!(!can_moderate(""));
    }

    #[test]
    fn channels_and_mentions_are_written_as_the_board_names_them() {
        assert_eq!(channel_id("71027"), "ch_71027");
        assert_eq!(channel_id("ch_71027"), "ch_71027");
        assert_eq!(mention("loupe"), "@loupe");
        assert_eq!(mention("@loupe"), "@loupe");
        assert_eq!(mention(""), "");
    }
}
