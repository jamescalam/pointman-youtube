//! What goes up to YouTube as bytes: a video, by its resumable upload (videos.insert with `uploadType=resumable`), and
//! a thumbnail (thumbnails.set).
//!
//! A video goes in chunks, each one request, so a dropped connection costs at most one chunk: after one, the session
//! is asked how much it has (`Content-Range: bytes */<size>`) and the upload carries on from there, waiting longer
//! after each failure in a row. The session's address is the command's checkpoint, so a node that restarts mid-upload
//! carries on too, while YouTube keeps the session (about a week).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use pointman_extension::Job;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::google::{json, send, Api, Failure};
use crate::words;

/// Failures in a row before an upload gives up.
const TRIES: u32 = 8;
/// A thumbnail's limit, as thumbnails.set has it.
pub const THUMBNAIL_MAX: u64 = 2 * 1024 * 1024;
/// Chunks are multiples of this, as YouTube asks, but the last.
pub const QUANTUM: u64 = 256 * 1024;

/// An upload under way: the command's checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub url: String,
    pub file: String,
    pub size: u64,
    pub modified: u64,
}

/// How far an upload has got, as YouTube says.
#[derive(Debug)]
pub enum Sent {
    /// It has this many bytes.
    At(u64),
    /// It has them all: the new video.
    Done(Value),
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

/// The bytes it has from a 308's `Range: bytes=0-<last>`: none without one.
fn range_end(r: &ureq::Response) -> u64 {
    r.header("Range")
        .and_then(|v| v.rsplit('-').next())
        .and_then(|n| n.trim().parse::<u64>().ok())
        .map(|last| last + 1)
        .unwrap_or(0)
}

fn sent(r: ureq::Response) -> Result<Sent, Failure> {
    match r.status() {
        308 => Ok(Sent::At(range_end(&r))),
        200 | 201 => Ok(Sent::Done(json(r)?)),
        status => {
            Err(Failure::Said { status, reason: String::new(), message: "an answer an upload doesn't have".into() })
        }
    }
}

fn failure(e: anyhow::Error) -> Result<Failure, anyhow::Error> {
    e.downcast::<Failure>()
}

/// A new upload session for the video's details (`snippet`, `status`): its address.
pub fn start(api: &Api, path: &Path, size: u64, details: &Value) -> anyhow::Result<String> {
    let url = api.upload_url("videos");
    let r = api.call(|auth| {
        send(
            api.agent
                .post(&url)
                .query("uploadType", "resumable")
                .query("part", "snippet,status")
                .set("Authorization", auth)
                .set("X-Upload-Content-Length", &size.to_string())
                .set("X-Upload-Content-Type", content_type(path))
                .send_json(details.clone()),
        )
    })?;
    r.header("Location")
        .map(String::from)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("YouTube didn't give the upload's address"))
}

/// How much of the video the session has.
pub fn status(api: &Api, session: &str, size: u64) -> anyhow::Result<Sent> {
    let r = api.call(|auth| {
        send(
            api.agent
                .put(session)
                .set("Authorization", auth)
                .set("Content-Range", &format!("bytes */{size}"))
                .send_bytes(&[]),
        )
    })?;
    Ok(sent(r)?)
}

/// One chunk, bytes `from..to` of the file.
fn chunk(api: &Api, session: &str, file: &mut File, from: u64, to: u64, size: u64) -> anyhow::Result<Sent> {
    let mut buf = vec![0u8; (to - from) as usize];
    file.seek(SeekFrom::Start(from))?;
    file.read_exact(&mut buf).context("couldn't read the video")?;
    let r = api.call(|auth| {
        send(
            api.agent
                .put(session)
                .set("Authorization", auth)
                .set("Content-Range", &format!("bytes {from}-{}/{size}", to - 1))
                .send_bytes(&buf),
        )
    })?;
    Ok(sent(r)?)
}

/// Whether to ask the session how far it got and try again: no answer, or YouTube's own trouble.
fn worth_again(f: &Failure) -> bool {
    match f {
        Failure::Lost(_) => true,
        Failure::Said { status, .. } => *status >= 500,
    }
}

/// A session YouTube no longer has.
fn gone(f: &Failure) -> bool {
    matches!(f, Failure::Said { status: 404 | 410, .. })
}

fn wait(job: &Job, failures: u32) -> anyhow::Result<()> {
    let total = Duration::from_secs((1u64 << (failures.saturating_sub(1)).min(6)).min(60));
    let mut waited = Duration::ZERO;
    while waited < total {
        job.check()?;
        std::thread::sleep(Duration::from_millis(100));
        waited += Duration::from_millis(100);
    }
    Ok(())
}

/// Upload a video: the new video's resource once YouTube has it all. `resume` is the checkpoint of an upload of this
/// file that was under way.
pub fn video(
    job: &Job,
    api: &Api,
    path: &Path,
    details: &Value,
    chunk_size: u64,
    resume: Option<Session>,
) -> anyhow::Result<Value> {
    let size = std::fs::metadata(path).with_context(|| format!("couldn't read {}", path.display()))?.len();
    if size == 0 {
        bail!("{} is empty", path.display());
    }
    let chunk_size = (chunk_size / QUANTUM).max(1) * QUANTUM;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut file = File::open(path).with_context(|| format!("couldn't open {}", path.display()))?;
    let this = Session { url: String::new(), file: path.display().to_string(), size, modified: modified(path) };

    let mut offset = 0;
    let mut session = resume.filter(|s| s.file == this.file && s.size == this.size && s.modified == this.modified);
    if let Some(s) = &session {
        job.progress(None, "asking YouTube how much of the upload it has");
        match status(api, &s.url, size).map_err(failure) {
            Ok(Sent::Done(video)) => return Ok(video),
            Ok(Sent::At(n)) => offset = n,
            Err(Ok(f)) if gone(&f) => session = None,
            Err(Ok(f)) if worth_again(&f) => {}
            Err(Ok(f)) => return Err(f.into()),
            Err(Err(e)) => return Err(e),
        }
    }
    let mut restarts = 0;
    let mut failures = 0;
    loop {
        let url = match &session {
            Some(s) => s.url.clone(),
            None => {
                job.progress(0.0, format!("starting the upload of {name} ({})", words::size(size)).as_str());
                let url = start(api, path, size, details)?;
                let s = Session { url: url.clone(), ..this.clone() };
                job.save(&s);
                session = Some(s);
                offset = 0;
                url
            }
        };
        job.check()?;
        let fraction = offset as f64 / size as f64;
        job.progress(fraction, format!("uploading {name}: {} of {}", words::size(offset), words::size(size)).as_str());
        // to the next chunk's boundary from the start, so every chunk but the last stays a multiple of 256 KiB, even
        // after YouTube kept part of one
        let to = ((offset / chunk_size + 1) * chunk_size).min(size);
        let got = chunk(api, &url, &mut file, offset, to, size).map_err(failure);
        let lost = match got {
            Ok(Sent::Done(video)) => return Ok(video),
            Ok(Sent::At(n)) => {
                if n > offset {
                    failures = 0;
                }
                offset = n.min(size);
                continue;
            }
            Err(Err(e)) => return Err(e),
            Err(Ok(f)) => f,
        };
        if gone(&lost) && restarts < 2 {
            restarts += 1;
            job.handle().log(format!("{name}: YouTube no longer has the upload ({lost}): starting it again"));
            session = None;
            continue;
        }
        if !worth_again(&lost) {
            return Err(lost.into());
        }
        failures += 1;
        if failures >= TRIES {
            bail!("gave up uploading {name} after {TRIES} failures in a row at {}: {lost}", words::size(offset));
        }
        job.handle().log(format!("{name}: {lost}; asking how much YouTube has (failure {failures} in a row)"));
        wait(job, failures)?;
        match status(api, &url, size).map_err(failure) {
            Ok(Sent::Done(video)) => return Ok(video),
            Ok(Sent::At(n)) => offset = n.min(size),
            Err(Ok(f)) if gone(&f) => session = None,
            Err(Ok(f)) if worth_again(&f) => {}
            Err(Ok(f)) => return Err(f.into()),
            Err(Err(e)) => return Err(e),
        }
    }
}

/// A thumbnail's MIME type, by its first bytes: JPEG or PNG, the two thumbnails.set takes.
pub fn image_type(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else {
        None
    }
}

/// A thumbnail's bytes and type, or why it won't do.
pub fn thumbnail_file(path: &Path) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let size = std::fs::metadata(path).with_context(|| format!("couldn't read {}", path.display()))?.len();
    if size > THUMBNAIL_MAX {
        bail!("{} is {}: a thumbnail is at most 2 MB", path.display(), words::size(size));
    }
    let bytes = std::fs::read(path)?;
    let kind = image_type(&bytes).ok_or_else(|| anyhow!("{} isn't a JPEG or PNG", path.display()))?;
    Ok((bytes, kind))
}

/// Set a video's thumbnail.
pub fn thumbnail(api: &Api, video: &str, bytes: &[u8], kind: &str) -> anyhow::Result<()> {
    let url = api.upload_url("thumbnails/set");
    api.call(|auth| {
        send(
            api.agent
                .post(&url)
                .query("videoId", video)
                .query("uploadType", "media")
                .set("Authorization", auth)
                .set("Content-Type", kind)
                .send_bytes(bytes),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_are_jpeg_or_png() {
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(b"GIF89a"), None);
    }

    #[test]
    fn videos_say_their_type() {
        assert_eq!(content_type(Path::new("a/final.MP4")), "video/mp4");
        assert_eq!(content_type(Path::new("cut.mov")), "video/quicktime");
        assert_eq!(content_type(Path::new("cut")), "application/octet-stream");
    }
}
