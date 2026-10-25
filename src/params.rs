//! The commands' params, as proto/youtube/v1/youtube.proto has them (`<Verb>Params`), read from the job's JSON, and the
//! limits its buf.validate rules give, checked here too: a job made through create_job may not have been checked
//! against the manifest's schema.

use anyhow::{bail, Context};
use regex::Regex;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Who can see a video (`Privacy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Privacy {
    Private,
    Unlisted,
    Public,
}

impl Privacy {
    pub fn word(self) -> &'static str {
        match self {
            Privacy::Private => "private",
            Privacy::Unlisted => "unlisted",
            Privacy::Public => "public",
        }
    }

    /// YouTube's `status.privacyStatus`.
    pub fn from_youtube(s: &str) -> Option<Self> {
        match s {
            "private" => Some(Privacy::Private),
            "unlisted" => Some(Privacy::Unlisted),
            "public" => Some(Privacy::Public),
            _ => None,
        }
    }
}

/// A video's category (`Category`): each one's number is YouTube's category id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    FilmAnimation = 1,
    AutosVehicles = 2,
    Music = 10,
    PetsAnimals = 15,
    Sports = 17,
    TravelEvents = 19,
    Gaming = 20,
    PeopleBlogs = 22,
    Comedy = 23,
    Entertainment = 24,
    NewsPolitics = 25,
    HowtoStyle = 26,
    Education = 27,
    ScienceTechnology = 28,
    NonprofitsActivism = 29,
}

const CATEGORIES: [Category; 15] = [
    Category::FilmAnimation,
    Category::AutosVehicles,
    Category::Music,
    Category::PetsAnimals,
    Category::Sports,
    Category::TravelEvents,
    Category::Gaming,
    Category::PeopleBlogs,
    Category::Comedy,
    Category::Entertainment,
    Category::NewsPolitics,
    Category::HowtoStyle,
    Category::Education,
    Category::ScienceTechnology,
    Category::NonprofitsActivism,
];

impl Category {
    /// YouTube's `snippet.categoryId`.
    pub fn id(self) -> String {
        (self as u32).to_string()
    }

    pub fn word(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
    }

    pub fn from_id(id: &str) -> Option<Self> {
        CATEGORIES.into_iter().find(|c| c.id() == id)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideosParams {
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub page: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoParams {
    pub video: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadParams {
    pub file: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub category: Option<Category>,
    pub privacy: Option<Privacy>,
    pub publish_at: Option<String>,
    pub made_for_kids: Option<bool>,
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default)]
    pub playlist: String,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateParams {
    pub video: String,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub category: Option<Category>,
    pub privacy: Option<Privacy>,
    pub publish_at: Option<String>,
    pub made_for_kids: Option<bool>,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThumbnailParams {
    pub video: String,
    pub file: String,
    #[serde(default)]
    pub confirm: bool,
}

/// A command's params, or what's wrong with them.
pub fn read<T: serde::de::DeserializeOwned>(params: &serde_json::Value) -> anyhow::Result<T> {
    let params = if params.is_null() { serde_json::json!({}) } else { params.clone() };
    serde_json::from_value(params).map_err(|e| anyhow::anyhow!("params: {e}"))
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

impl UploadParams {
    pub fn check(&self) -> anyhow::Result<()> {
        check_title(&self.title)?;
        check_description(&self.description)?;
        check_tags(&self.tags)?;
        check_playlist(&self.playlist)?;
        if let Some(at) = &self.publish_at {
            check_publish_at(at)?;
            if self.privacy.is_some_and(|p| p != Privacy::Private) {
                bail!("publish_at: only a private video is scheduled, and privacy is {}", self.privacy.unwrap().word());
            }
        }
        Ok(())
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
        if let Some(at) = &self.publish_at {
            check_publish_at(at)?;
            if self.privacy.is_some_and(|p| p != Privacy::Private) {
                bail!("publish_at: only a private video is scheduled, and privacy is {}", self.privacy.unwrap().word());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_count_as_youtube_counts_them() {
        let tags: Vec<String> = ["ai", "claude code", "rust"].map(String::from).into();
        assert_eq!(tags_length(&tags), 2 + (11 + 2) + 4 + 2);
        let long: Vec<String> = (0..60).map(|i| format!("tag number {i:02}")).collect();
        assert!(check_tags(&long).is_err());
    }

    #[test]
    fn categories_are_youtubes_ids() {
        assert_eq!(Category::ScienceTechnology.id(), "28");
        assert_eq!(Category::from_id("27"), Some(Category::Education));
        assert_eq!(Category::from_id("30"), None, "Movies: YouTube lists it, a video can't take it");
        assert_eq!(Category::PeopleBlogs.word(), "people_blogs");
        let read: UploadParams =
            read(&serde_json::json!({"file": "a.mp4", "title": "t", "category": "science_technology"})).unwrap();
        assert_eq!(read.category, Some(Category::ScienceTechnology));
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
        assert!(read::<UpdateParams>(&serde_json::json!({"video": "dQw4w9WgXcQ", "titel": "x"})).is_err());
    }
}
