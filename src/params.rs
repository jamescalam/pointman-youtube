//! The commands' params are proto/youtube/v1/youtube.proto's messages (`v1::UploadParams`…), which the SDK's typed
//! handlers read: an unknown field, a missing required one or a word an enum doesn't have fails the command before it
//! runs. What a JSON Schema can't say (a description's 5,000 bytes, tags as YouTube counts them, a time in the
//! future), and the limits again for a job that didn't go through the manifest's schema, are checked here.

use anyhow::{bail, Context};
use buffa::{EnumValue, Enumeration};
use buffa_types::google::protobuf::Timestamp;
use regex::Regex;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::v1::{Category, Privacy, UpdateParams, UploadParams};

impl Privacy {
    /// Its word, as the manifest and YouTube's `status.privacyStatus` say it.
    pub fn word(self) -> &'static str {
        match self {
            Privacy::PRIVACY_UNLISTED => "unlisted",
            Privacy::PRIVACY_PUBLIC => "public",
            Privacy::PRIVACY_PRIVATE | Privacy::PRIVACY_UNSPECIFIED => "private",
        }
    }

    pub fn from_youtube(s: &str) -> Option<Self> {
        match s {
            "private" => Some(Privacy::PRIVACY_PRIVATE),
            "unlisted" => Some(Privacy::PRIVACY_UNLISTED),
            "public" => Some(Privacy::PRIVACY_PUBLIC),
            _ => None,
        }
    }
}

/// A known value of an enum field: None for one left out, unspecified, or a number this build doesn't know.
pub fn known<E: Enumeration + Copy>(v: &EnumValue<E>) -> Option<E> {
    v.as_known().filter(|e| e.to_i32() != 0)
}

impl Category {
    /// YouTube's `snippet.categoryId`: each value's number.
    pub fn id(self) -> String {
        self.to_i32().to_string()
    }

    /// Its word, as the manifest says it: `science_technology`.
    pub fn word(self) -> String {
        self.proto_name().trim_start_matches("CATEGORY_").to_lowercase()
    }

    /// One a video can be given, from YouTube's id.
    pub fn from_id(id: &str) -> Option<Self> {
        id.parse().ok().and_then(Category::from_i32).filter(|c| c.to_i32() != 0)
    }
}

/// A Timestamp as RFC 3339, in UTC.
pub fn rfc3339(t: &Timestamp) -> String {
    OffsetDateTime::from_unix_timestamp(t.seconds).ok().and_then(|d| d.format(&Rfc3339).ok()).unwrap_or_default()
}

/// An RFC 3339 time (as YouTube gives them) as a Timestamp.
pub fn timestamp(at: &str) -> Option<Timestamp> {
    let t = OffsetDateTime::parse(at, &Rfc3339).ok()?;
    Some(Timestamp { seconds: t.unix_timestamp(), nanos: t.nanosecond() as i32, ..Default::default() })
}

pub fn check_video(id: &str) -> anyhow::Result<()> {
    if !Regex::new("^[A-Za-z0-9_-]{11}$").expect("compiles").is_match(id) {
        bail!("video: {id:?} isn't a video id (the 11 characters after watch?v= or youtu.be/)");
    }
    Ok(())
}

pub fn check_title(title: &str) -> anyhow::Result<()> {
    let n = title.chars().count();
    if n == 0 || n > 100 {
        bail!("title: 1 to 100 characters, not {n}");
    }
    if title.contains(['<', '>']) {
        bail!("title: YouTube refuses < and >");
    }
    Ok(())
}

pub fn check_description(description: &str) -> anyhow::Result<()> {
    if description.len() > 5000 {
        bail!("description: at most 5,000 bytes, not {}", description.len());
    }
    if description.contains(['<', '>']) {
        bail!("description: YouTube refuses < and >");
    }
    Ok(())
}

/// Tags as YouTube counts them: each tag's characters, 2 more for one with a space (its quotes), and a comma between.
pub fn tags_length(tags: &[String]) -> usize {
    let each: usize = tags.iter().map(|t| t.chars().count() + if t.contains(' ') { 2 } else { 0 }).sum();
    each + tags.len().saturating_sub(1)
}

pub fn check_tags(tags: &[String]) -> anyhow::Result<()> {
    if tags.len() > 100 {
        bail!("tags: at most 100, not {}", tags.len());
    }
    if let Some(t) = tags.iter().find(|t| t.trim().is_empty() || t.contains(['<', '>', ','])) {
        bail!("tags: {t:?} is empty or has < > or a comma");
    }
    let n = tags_length(tags);
    if n > 500 {
        bail!("tags: {n} characters as YouTube counts them (a comma between each, 2 more for a tag with a space), and it takes 500");
    }
    Ok(())
}

/// `publish_at`: RFC 3339, in the future, as YouTube's `status.publishAt` takes it.
pub fn check_publish_at(at: &str) -> anyhow::Result<String> {
    let when = OffsetDateTime::parse(at.trim(), &Rfc3339)
        .with_context(|| format!("publish_at: {at:?} isn't an RFC 3339 time (2026-11-02T15:00:00Z)"))?;
    if when <= OffsetDateTime::now_utc() {
        bail!("publish_at: {at} has passed: give a time in the future");
    }
    Ok(when.to_offset(time::UtcOffset::UTC).format(&Rfc3339).expect("formats"))
}

pub fn check_playlist(id: &str) -> anyhow::Result<()> {
    if id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        bail!("playlist: {id:?} isn't a playlist id (PL…)");
    }
    Ok(())
}

/// `publish_at` as YouTube takes it, if given: in the future, and only for a video that ends up private.
fn publish_at(at: Option<&Timestamp>, privacy: Option<Privacy>) -> anyhow::Result<Option<String>> {
    let Some(at) = at else { return Ok(None) };
    let at = check_publish_at(&rfc3339(at))?;
    if let Some(p) = privacy.filter(|p| *p != Privacy::PRIVACY_PRIVATE) {
        bail!("publish_at: only a private video is scheduled, and privacy is {}", p.word());
    }
    Ok(Some(at))
}

impl UploadParams {
    pub fn check(&self) -> anyhow::Result<()> {
        check_title(&self.title)?;
        check_description(&self.description)?;
        check_tags(&self.tags)?;
        check_playlist(&self.playlist)?;
        self.publish_at_checked()?;
        Ok(())
    }

    /// Who can see it: private unless it says.
    pub fn privacy_or_private(&self) -> Privacy {
        known(&self.privacy).unwrap_or(Privacy::PRIVACY_PRIVATE)
    }

    pub fn publish_at_checked(&self) -> anyhow::Result<Option<String>> {
        publish_at(self.publish_at.as_option(), Some(self.privacy_or_private()))
    }
}

impl UpdateParams {
    pub fn check(&self) -> anyhow::Result<()> {
        check_video(&self.video)?;
        if let Some(t) = &self.title {
            check_title(t)?;
        }
        if let Some(d) = &self.description {
            check_description(d)?;
        }
        check_tags(&self.tags)?;
        self.publish_at_checked()?;
        Ok(())
    }

    /// A new privacy, if it asks for one.
    pub fn new_privacy(&self) -> Option<Privacy> {
        self.privacy.as_ref().and_then(known)
    }

    pub fn new_category(&self) -> Option<Category> {
        self.category.as_ref().and_then(known)
    }

    pub fn publish_at_checked(&self) -> anyhow::Result<Option<String>> {
        publish_at(self.publish_at.as_option(), self.new_privacy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointman_extension::typed::from_value;

    #[test]
    fn tags_count_as_youtube_counts_them() {
        let tags: Vec<String> = ["ai", "claude code", "rust"].map(String::from).into();
        assert_eq!(tags_length(&tags), 2 + (11 + 2) + 4 + 2);
        let long: Vec<String> = (0..60).map(|i| format!("tag number {i:02}")).collect();
        assert!(check_tags(&long).is_err());
    }

    #[test]
    fn categories_are_youtubes_ids() {
        assert_eq!(Category::CATEGORY_SCIENCE_TECHNOLOGY.id(), "28");
        assert_eq!(Category::from_id("27"), Some(Category::CATEGORY_EDUCATION));
        assert_eq!(Category::from_id("30"), None, "Movies: YouTube lists it, a video can't take it");
        assert_eq!(Category::from_id("0"), None);
        assert_eq!(Category::CATEGORY_PEOPLE_BLOGS.word(), "people_blogs");
        let read: UploadParams =
            from_value(serde_json::json!({"file": "a.mp4", "title": "t", "category": "science_technology"})).unwrap();
        assert_eq!(known(&read.category), Some(Category::CATEGORY_SCIENCE_TECHNOLOGY));
    }

    #[test]
    fn limits() {
        assert!(check_title(&"x".repeat(100)).is_ok());
        assert!(check_title(&"x".repeat(101)).is_err());
        assert!(check_title("a <b>").is_err());
        assert!(check_description(&"é".repeat(2501)).is_err(), "bytes, not characters");
        assert!(check_video("dQw4w9WgXcQ").is_ok());
        assert!(check_video("https://youtu.be/dQw4w9WgXcQ").is_err());
        assert!(check_publish_at("2020-01-01T00:00:00Z").is_err());
        assert_eq!(check_publish_at("2999-01-01T09:00:00+08:00").unwrap(), "2999-01-01T01:00:00Z");
        let p: UpdateParams =
            from_value(serde_json::json!({"video": "dQw4w9WgXcQ", "publish_at": "2999-01-01T09:00:00+08:00"})).unwrap();
        assert_eq!(p.publish_at_checked().unwrap().as_deref(), Some("2999-01-01T01:00:00Z"));
        assert_eq!(timestamp("2026-10-03T15:04:00Z").map(|t| rfc3339(&t)).as_deref(), Some("2026-10-03T15:04:00Z"));
    }
}
