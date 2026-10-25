//! The channel's videos and their details through the Data API: listing them, reading one, and changing one.
//!
//! **Changing one** (videos.update) replaces every mutable property in each part it's sent: a `snippet` sent with only a
//! title would clear the description and tags. So a change reads the video first (videos.list), changes only the
//! fields asked for in what it read, and sends back only the parts with a change in them.

use anyhow::bail;
use serde_json::{json, Map, Value};

use crate::google::Api;
use crate::params::{check_publish_at, Category, Privacy, UpdateParams};

pub fn url(video: &str) -> String {
    format!("https://youtu.be/{video}")
}

/// One video's resource with the parts asked for (`snippet,status`), or an error saying there's none.
pub fn fetch(api: &Api, video: &str, parts: &str) -> anyhow::Result<Value> {
    let got = api.get("videos", &[("part", parts), ("id", video)])?;
    match got["items"].as_array().and_then(|i| i.first()) {
        Some(v) => Ok(v.clone()),
        None => bail!("YouTube has no video {video} that this channel can see"),
    }
}

/// A video's privacy, as its `status` says.
pub fn privacy(video: &Value) -> Option<Privacy> {
    video["status"]["privacyStatus"].as_str().and_then(Privacy::from_youtube)
}

/// One row of youtube.videos (`Video`).
pub fn row(v: &Value) -> Value {
    let id = v["id"].as_str().unwrap_or_default();
    let mut row = json!({
        "id": id,
        "title": v["snippet"]["title"],
        "privacy": v["status"]["privacyStatus"],
        "published_at": v["snippet"]["publishedAt"],
        "views": v["statistics"]["viewCount"].as_str().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0),
        "url": url(id),
    });
    if let Some(at) = v["status"]["publishAt"].as_str() {
        row["publish_at"] = json!(at);
    }
    row
}

/// The channel's videos, newest first: a page of rows, the next page's token, and how many there are in all.
pub fn videos(api: &Api, page_size: u32, page: &str) -> anyhow::Result<(Vec<Value>, String, u64)> {
    let channels = api.get("channels", &[("part", "contentDetails,statistics"), ("mine", "true")])?;
    let Some(channel) = channels["items"].as_array().and_then(|i| i.first()) else {
        bail!("the signed-in Google account has no YouTube channel");
    };
    let uploads = channel["contentDetails"]["relatedPlaylists"]["uploads"].as_str().unwrap_or_default();
    if uploads.is_empty() {
        return Ok((vec![], String::new(), 0));
    }
    let size = page_size.clamp(1, 50).to_string();
    let mut query = vec![("part", "contentDetails"), ("playlistId", uploads), ("maxResults", size.as_str())];
    if !page.is_empty() {
        query.push(("pageToken", page));
    }
    let items = api.get("playlistItems", &query)?;
    let total = items["pageInfo"]["totalResults"]
        .as_u64()
        .or_else(|| channel["statistics"]["videoCount"].as_str().and_then(|n| n.parse().ok()))
        .unwrap_or(0);
    let next = items["nextPageToken"].as_str().unwrap_or_default().to_string();
    let ids: Vec<&str> = items["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["contentDetails"]["videoId"].as_str())
        .collect();
    if ids.is_empty() {
        return Ok((vec![], next, total));
    }
    let joined = ids.join(",");
    let found = api.get("videos", &[("part", "snippet,status,statistics"), ("id", &joined), ("maxResults", "50")])?;
    let by_id: Map<String, Value> = found["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str().map(|id| (id.to_string(), v.clone())))
        .collect();
    // in the uploads playlist's order (newest first); one deleted since is left out
    let rows = ids.iter().filter_map(|id| by_id.get(*id)).map(row).collect();
    Ok((rows, next, total))
}

/// Read-only fields of each part: left out of what's sent back (YouTube ignores them, and they aren't the change's).
const READ_ONLY: &[(&str, &[&str])] = &[
    ("snippet", &["publishedAt", "channelId", "channelTitle", "thumbnails", "liveBroadcastContent", "localized"]),
    ("status", &["uploadStatus", "failureReason", "rejectionReason", "madeForKids"]),
];

/// A change, worked out from the video as YouTube has it now.
#[derive(Debug, PartialEq)]
pub struct Change {
    /// The parts to send (`snippet`, `status`), each whole.
    pub parts: Vec<&'static str>,
    /// videos.update's body: the id and those parts.
    pub body: Value,
    /// What changes, in the params' words.
    pub changed: Vec<&'static str>,
    /// Privacy before, and after.
    pub was: Option<Privacy>,
    pub becomes: Option<Privacy>,
}

impl Change {
    /// Whether the owner must have said yes: the video is public, goes public, or is scheduled to.
    pub fn needs_confirm(&self) -> bool {
        self.was == Some(Privacy::Public)
            || self.becomes == Some(Privacy::Public)
            || self.changed.contains(&"publish_at")
    }
}

/// Merge what's asked into the video (with its `snippet` and `status`): every field not asked for stays as it is.
pub fn merge(video: &Value, p: &UpdateParams) -> anyhow::Result<Change> {
    let mut snippet = video["snippet"].as_object().cloned().unwrap_or_default();
    let mut status = video["status"].as_object().cloned().unwrap_or_default();
    let was = privacy(video);
    let mut changed: Vec<&'static str> = vec![];
    fn set(
        part: &mut Map<String, Value>,
        key: &str,
        value: Value,
        name: &'static str,
        changed: &mut Vec<&'static str>,
    ) {
        if part.get(key) != Some(&value) {
            part.insert(key.to_string(), value);
            changed.push(name);
        }
    }
    if let Some(t) = &p.title {
        set(&mut snippet, "title", json!(t), "title", &mut changed);
    }
    if let Some(d) = &p.description {
        set(&mut snippet, "description", json!(d), "description", &mut changed);
    }
    if !p.tags.is_empty() {
        set(&mut snippet, "tags", json!(p.tags), "tags", &mut changed);
    }
    if let Some(c) = p.category {
        set(&mut snippet, "categoryId", json!(c.id()), "category", &mut changed);
    }
    if let Some(at) = &p.publish_at {
        let at = check_publish_at(at)?;
        let ends_private = match p.privacy {
            Some(pr) => pr == Privacy::Private,
            None => was == Some(Privacy::Private),
        };
        if !ends_private {
            bail!(
                "publish_at schedules a private video, and this one is {}: give privacy=private with it",
                was.map(Privacy::word).unwrap_or("of unknown privacy")
            );
        }
        set(&mut status, "publishAt", json!(at), "publish_at", &mut changed);
    }
    if let Some(pr) = p.privacy {
        set(&mut status, "privacyStatus", json!(pr.word()), "privacy", &mut changed);
        // a schedule is only a private video's: one made unlisted or public now loses it
        if pr != Privacy::Private && status.remove("publishAt").is_some() && !changed.contains(&"publish_at") {
            changed.push("publish_at");
        }
    }
    if let Some(k) = p.made_for_kids {
        set(&mut status, "selfDeclaredMadeForKids", json!(k), "made_for_kids", &mut changed);
    }
    let becomes = status.get("privacyStatus").and_then(Value::as_str).and_then(Privacy::from_youtube);
    let in_snippet = ["title", "description", "tags", "category"];
    let mut parts = vec![];
    let mut body = json!({"id": video["id"]});
    for (name, mut part) in [("snippet", snippet), ("status", status)] {
        let touched = changed.iter().any(|c| in_snippet.contains(c) == (name == "snippet"));
        if !touched {
            continue;
        }
        for (_, keys) in READ_ONLY.iter().filter(|(n, _)| *n == name) {
            for k in *keys {
                part.remove(*k);
            }
        }
        body[name] = Value::Object(part);
        parts.push(name);
    }
    Ok(Change { parts, body, changed, was, becomes })
}

/// Send a change: the video as YouTube has it after.
pub fn update(api: &Api, change: &Change) -> anyhow::Result<Value> {
    api.send_json("PUT", "videos", &[("part", &change.parts.join(","))], &change.body)
}

/// Add a video to a playlist, at its end.
pub fn add_to_playlist(api: &Api, playlist: &str, video: &str) -> anyhow::Result<()> {
    let body = json!({"snippet": {"playlistId": playlist, "resourceId": {"kind": "youtube#video", "videoId": video}}});
    api.send_json("POST", "playlistItems", &[("part", "snippet")], &body)?;
    Ok(())
}

/// A video's category in the params' words, or YouTube's id when it's one a video can't be given.
pub fn category_word(id: &str) -> String {
    Category::from_id(id).map(Category::word).unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::read;

    fn video(privacy: &str) -> Value {
        json!({
            "id": "abcdefghijk",
            "snippet": {"title": "Old", "description": "Keep me", "tags": ["a", "b"], "categoryId": "28",
                        "defaultLanguage": "en", "publishedAt": "2026-10-01T00:00:00Z", "channelId": "UC1",
                        "thumbnails": {"default": {"url": "x"}}},
            "status": {"privacyStatus": privacy, "embeddable": true, "license": "youtube",
                       "selfDeclaredMadeForKids": false, "madeForKids": false, "uploadStatus": "processed"}
        })
    }

    fn params(v: Value) -> UpdateParams {
        read(&v).unwrap()
    }

    #[test]
    fn a_title_alone_keeps_the_rest_of_the_snippet_and_leaves_status_unsent() {
        let c = merge(&video("private"), &params(json!({"video": "abcdefghijk", "title": "New"}))).unwrap();
        assert_eq!(c.parts, vec!["snippet"]);
        assert_eq!(c.changed, vec!["title"]);
        let s = &c.body["snippet"];
        assert_eq!(s["title"], "New");
        assert_eq!(s["description"], "Keep me");
        assert_eq!(s["tags"], json!(["a", "b"]));
        assert_eq!(s["categoryId"], "28");
        assert_eq!(s["defaultLanguage"], "en");
        assert!(s.get("publishedAt").is_none() && s.get("thumbnails").is_none(), "read-only fields aren't sent");
        assert!(c.body.get("status").is_none());
        assert!(!c.needs_confirm());
    }

    #[test]
    fn a_public_video_or_going_public_or_a_schedule_needs_confirm() {
        let v = json!({"video": "abcdefghijk", "description": "x"});
        assert!(merge(&video("public"), &params(v)).unwrap().needs_confirm());
        let v = json!({"video": "abcdefghijk", "privacy": "public"});
        let c = merge(&video("unlisted"), &params(v)).unwrap();
        assert!(c.needs_confirm());
        assert_eq!(c.parts, vec!["status"]);
        assert_eq!(c.body["status"]["embeddable"], true, "the rest of status is kept");
        assert!(c.body["status"].get("uploadStatus").is_none());
        let v = json!({"video": "abcdefghijk", "publish_at": "2999-01-01T00:00:00Z"});
        assert!(merge(&video("private"), &params(v)).unwrap().needs_confirm());
    }

    #[test]
    fn a_schedule_is_only_a_private_videos() {
        let v = json!({"video": "abcdefghijk", "publish_at": "2999-01-01T00:00:00Z"});
        assert!(merge(&video("unlisted"), &params(v)).is_err());
        let v = json!({"video": "abcdefghijk", "publish_at": "2999-01-01T00:00:00Z", "privacy": "private"});
        let c = merge(&video("unlisted"), &params(v)).unwrap();
        assert_eq!(c.body["status"]["publishAt"], "2999-01-01T00:00:00Z");
        assert_eq!(c.body["status"]["privacyStatus"], "private");
        let mut scheduled = video("private");
        scheduled["status"]["publishAt"] = json!("2999-01-01T00:00:00Z");
        let c = merge(&scheduled, &params(json!({"video": "abcdefghijk", "privacy": "unlisted"}))).unwrap();
        assert!(c.body["status"].get("publishAt").is_none());
        assert_eq!(c.changed, vec!["privacy", "publish_at"]);
    }

    #[test]
    fn the_same_value_again_is_no_change() {
        let c = merge(&video("private"), &params(json!({"video": "abcdefghijk", "title": "Old"}))).unwrap();
        assert!(c.changed.is_empty() && c.parts.is_empty());
    }
}
