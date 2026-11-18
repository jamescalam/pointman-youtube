//! The extension's process: the node starts it (`bin/youtube`, from the extension's folder) and talks to it over stdin
//! and stdout. It lists the channel's videos, uploads new ones and changes their details, and signs channels in from
//! the apps. Nothing reaches the channel without a sign-in in the vault, and nothing is uploaded, and no public video
//! changed, without `confirm`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use pointman_extension::protocol::{
    BoardPostRequest, CredentialsCodeRequest, CredentialsCodeResponse, CredentialsSignInRequest,
    CredentialsWaitRequest, CredentialsWaitResponse,
};
use pointman_extension::v1::{Block, ThreadRef};
use pointman_extension::{blocks, Extension, Handle, Job};
use regex::Regex;
use serde_json::json;
use youtube::google::{self, Api, Endpoints, OAuthClient};
use youtube::params::{check_video, known, rfc3339, timestamp};
use youtube::signin::{self, SignIns};
use youtube::v1::{
    AddParams, AddResult, Category, ChatMessage, ChatParams, ChatRemoveParams, ChatRemoveResult, ChatResult,
    ChatSendParams, ChatSendResult, ChatTimeoutParams, ChatTimeoutResult, ChatWatchParams, ChatWatchResult,
    ClientParams, ClientResult, LiveParams, LiveResult, Privacy, Settings, ThumbnailParams, ThumbnailResult,
    UpdateParams, UpdateResult, UploadParams, UploadResult, VideoParams, VideoResult, VideosParams, VideosResult,
};
use youtube::{channel, chat, upload, words, CLIENT_ITEM};

struct Cached {
    name: String,
    token: String,
    until: Instant,
    /// The scopes Google says the token has.
    scopes: String,
}

struct Ext {
    h: Handle,
    agent: ureq::Agent,
    ends: Endpoints,
    cached: Mutex<Option<Cached>>,
}

impl Ext {
    fn machine(&self) -> String {
        self.h.machine()
    }

    /// The vault item of the channel's sign-in to use: the call's `account`, else the setting's, else the only one.
    /// With several signed in and none named it stops and lists them, rather than take one by chance.
    fn account(&self, asked: &str) -> anyhow::Result<String> {
        let mut channels: Vec<String> = self.h.secrets("youtube.")?.into_iter().filter(|n| n != CLIENT_ITEM).collect();
        channels.sort();
        let have = if channels.is_empty() { "none".to_string() } else { channels.join(", ") };
        let asked = asked.trim();
        let (want, from) = if asked.is_empty() {
            (self.h.typed_settings::<Settings>()?.account.trim().to_string(), "the account setting")
        } else {
            (asked.to_string(), "account")
        };
        if !want.is_empty() {
            let name = format!("youtube.{want}");
            if channels.contains(&name) {
                return Ok(name);
            }
            bail!(
                "no YouTube sign-in {name} on {} ({from}); it has {have}: pass account=<label> of one of them, or sign it in with /youtube add",
                self.machine()
            );
        }
        match channels.len() {
            1 => Ok(channels.remove(0)),
            0 => {
                let client = match google::default_client() {
                    Some(_) => "",
                    None => "add the Google OAuth client (/youtube client) if it isn't there, then ",
                };
                bail!(
                    "no YouTube channel is signed in on {}: {client}type /youtube add in the app and tap Sign in",
                    self.machine()
                )
            }
            _ => bail!(
                "{} YouTube channels are signed in on {} ({have}) and none is chosen: pass account=<label> (the part after youtube.) with this command, or set the account setting",
                channels.len(),
                self.machine()
            ),
        }
    }

    /// The node's own `youtube.client` when it has one, else the one the release was built with.
    fn client(&self) -> anyhow::Result<OAuthClient> {
        let Ok(value) = self.h.secret(CLIENT_ITEM) else {
            return google::default_client().ok_or_else(|| {
                anyhow!(
                    "no Google OAuth client on {}: type /youtube client in the app and paste a TVs and Limited Input \
                     client's ID and secret",
                    self.machine()
                )
            });
        };
        OAuthClient::parse(&value).ok_or_else(|| {
            anyhow!(
                "{CLIENT_ITEM} isn't a client ID and secret (…apps.googleusercontent.com, a space, then the secret)"
            )
        })
    }

    /// An access token for the channel, made again when it's within a minute of running out, or `fresh`.
    fn token(&self, asked: &str, fresh: bool) -> anyhow::Result<String> {
        let name = self.account(asked)?;
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) =
            cached.as_ref().filter(|c| c.name == name && !fresh && c.until > Instant::now() + Duration::from_secs(60))
        {
            return Ok(c.token.clone());
        }
        let value = self.h.secret(&name)?;
        let refresh = Regex::new(r"1//[A-Za-z0-9_-]+")
            .expect("compiles")
            .find(&value)
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                anyhow!("{name} isn't a Google refresh token: sign the channel in again with /youtube add")
            })?;
        let (token, lasts, scopes) = google::access_token(&self.agent, &self.ends, &self.client()?, &refresh)?;
        *cached = Some(Cached { name, token: token.clone(), until: Instant::now() + lasts, scopes });
        Ok(token)
    }

    /// The scopes the sign-in's token has, as Google gave them with it.
    fn scopes(&self, asked: &str) -> anyhow::Result<String> {
        self.token(asked, false)?;
        let cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        Ok(cached.as_ref().map(|c| c.scopes.clone()).unwrap_or_default())
    }

    /// A post in a thread, as the extension's robot: its id.
    fn post(&self, thread: &str, text: String, blocks: Vec<Block>) -> anyhow::Result<String> {
        let ask = BoardPostRequest {
            thread: ThreadRef { id: chat::thread_id(thread), ..Default::default() }.into(),
            text,
            blocks,
            ..Default::default()
        };
        let answer = self.h.request(&ask)?;
        Ok(answer.post.as_option().map(|p| p.id.clone()).unwrap_or_default())
    }

    fn api<T>(&self, account: &str, f: impl FnOnce(&Api) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let token = |fresh: bool| self.token(account, fresh);
        f(&Api { agent: &self.agent, ends: &self.ends, token: &token })
    }

    /// A file a command was given: a path in the folders the node lets the extension use (a board file the node
    /// fetched for `file` is in its state folder).
    fn file(&self, given: &str, what: &str) -> anyhow::Result<PathBuf> {
        let given = given.trim();
        if given.is_empty() {
            bail!("{what} is missing");
        }
        if given.contains("://") {
            bail!("{what}: {given} wasn't fetched by this node: give a board file (pointman://asset/<id>) or a path on it");
        }
        let path = self.h.allowed(given)?;
        if !path.is_file() {
            bail!("{what}: {} isn't a file", path.display());
        }
        Ok(path)
    }
}

fn name_of(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn videos(x: &Ext, _: &Job, p: VideosParams) -> anyhow::Result<VideosResult> {
    let (videos, next, total) = x.api(&p.account, |api| channel::videos(api, p.page_size, &p.page))?;
    let mut text = if videos.is_empty() {
        "No videos on the channel.".to_string()
    } else {
        format!("{} of the channel's {} videos, newest first:", videos.len(), words::count(total as u64))
    };
    for v in &videos {
        let when = match v.publish_at.as_option() {
            Some(at) => format!("goes public {}", words::day(&rfc3339(at), true)),
            None => v.published_at.as_option().map(|at| words::day(&rfc3339(at), false)).unwrap_or_default(),
        };
        let privacy = known(&v.privacy).map(Privacy::word).unwrap_or_default();
        let views = words::count(v.views as u64);
        text.push_str(&format!("\n- {} ({privacy}, {views} views, {when}): {}", v.title, v.id));
    }
    if !next.is_empty() {
        text.push_str(&format!("\nMore: youtube.videos with page={next}"));
    }
    Ok(VideosResult { text, videos, next_page: next, total: total as i32, ..Default::default() })
}

fn video(x: &Ext, _: &Job, p: VideoParams) -> anyhow::Result<VideoResult> {
    check_video(&p.video)?;
    let v = x.api(&p.account, |api| channel::fetch(api, &p.video, "snippet,status,statistics,contentDetails"))?;
    let (s, st) = (&v["snippet"], &v["status"]);
    let text = |k: &str| s[k].as_str().unwrap_or_default().to_string();
    let mut out = VideoResult {
        url: channel::url(&p.video),
        title: text("title"),
        description: text("description"),
        tags: s["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect(),
        category: channel::category_word(s["categoryId"].as_str().unwrap_or_default()),
        privacy: channel::privacy(&v).unwrap_or_default().into(),
        published_at: s["publishedAt"].as_str().and_then(timestamp).into(),
        publish_at: st["publishAt"].as_str().and_then(timestamp).into(),
        made_for_kids: st["selfDeclaredMadeForKids"].as_bool(),
        views: channel::views(&v),
        duration: v["contentDetails"]["duration"].as_str().unwrap_or_default().to_string(),
        video: p.video,
        ..Default::default()
    };
    let schedule =
        st["publishAt"].as_str().map(|at| format!(", goes public {}", words::day(at, true))).unwrap_or_default();
    out.text = format!(
        "\"{}\" ({}{schedule}, {} views): {}, {} tags, {} characters of description.",
        out.title,
        st["privacyStatus"].as_str().unwrap_or_default(),
        words::count(out.views as u64),
        out.url,
        out.tags.len(),
        out.description.chars().count(),
    );
    Ok(out)
}

fn upload_video(x: &Ext, job: &Job, p: UploadParams) -> anyhow::Result<UploadResult> {
    let privacy = p.privacy_or_private();
    if !p.confirm {
        bail!(
            "not uploaded: youtube.upload needs confirm=true, given only once the channel's owner has said yes to \
             uploading {} as \"{}\" ({})",
            p.file,
            p.title,
            privacy.word()
        );
    }
    p.check()?;
    let path = x.file(&p.file, "file")?;
    let thumb = match p.thumbnail.trim() {
        "" => None,
        t if t.contains("://") => bail!(
            "thumbnail: {t} is a board file, which the node fetches only for youtube.thumbnail's file: upload \
             without it, then run youtube.thumbnail with the new video's id"
        ),
        t => Some(upload::thumbnail_file(&x.h.allowed(t)?)?),
    };
    let publish_at = p.publish_at_checked()?;
    let category = known(&p.category).unwrap_or(Category::CATEGORY_PEOPLE_BLOGS).id();
    let mut snippet = json!({"title": p.title, "description": p.description, "categoryId": category});
    if !p.tags.is_empty() {
        snippet["tags"] = json!(p.tags);
    }
    let mut status = json!({"privacyStatus": privacy.word()});
    if let Some(at) = &publish_at {
        status["publishAt"] = json!(at);
    }
    if let Some(k) = p.made_for_kids {
        status["selfDeclaredMadeForKids"] = json!(k);
    }
    let details = json!({"snippet": snippet, "status": status});
    let resume = job.checkpoint.clone().and_then(|c| serde_json::from_value::<upload::Session>(c).ok());
    let settings: Settings = x.h.typed_settings()?;
    let chunk = (settings.chunk_mib.clamp(1, 256) as u64) << 20;
    let size = std::fs::metadata(&path)?.len();
    let started = Instant::now();

    let made = x.api(&p.account, |api| upload::video(job, api, &path, &details, chunk, resume))?;
    let id = made["id"]
        .as_str()
        .filter(|i| !i.is_empty())
        .ok_or_else(|| anyhow!("YouTube didn't say the new video's id"))?
        .to_string();
    let mut text = format!(
        "Uploaded {} ({}, in {} s) as \"{}\": {}, {}",
        name_of(&path),
        words::size(size),
        started.elapsed().as_secs(),
        p.title,
        channel::url(&id),
        privacy.word()
    );
    if let Some(at) = &publish_at {
        text.push_str(&format!(", going public {}", words::day(at, true)));
    }
    text.push('.');
    let mut thumbnail = false;
    if let Some((bytes, kind)) = thumb {
        job.progress(1.0, "setting the thumbnail");
        match x.api(&p.account, |api| upload::thumbnail(api, &id, &bytes, kind)) {
            Ok(()) => {
                thumbnail = true;
                text.push_str(" Thumbnail set.");
            }
            Err(e) => text.push_str(&format!(" The thumbnail wasn't set ({e:#}): try youtube.thumbnail.")),
        }
    }
    let mut playlist = String::new();
    if !p.playlist.is_empty() {
        job.progress(1.0, "adding it to the playlist");
        match x.api(&p.account, |api| channel::add_to_playlist(api, &p.playlist, &id)) {
            Ok(()) => {
                playlist = p.playlist.clone();
                text.push_str(&format!(" Added to playlist {}.", p.playlist));
            }
            Err(e) => text.push_str(&format!(" It wasn't added to playlist {} ({e:#}).", p.playlist)),
        }
    }
    text.push_str(" YouTube processes it before it plays in full.");
    Ok(UploadResult {
        text,
        url: channel::url(&id),
        video: id,
        privacy: privacy.into(),
        publish_at: publish_at.as_deref().and_then(timestamp).into(),
        bytes: size as i64,
        thumbnail,
        playlist,
        ..Default::default()
    })
}

fn update(x: &Ext, _: &Job, p: UpdateParams) -> anyhow::Result<UpdateResult> {
    p.check()?;
    let now = x.api(&p.account, |api| channel::fetch(api, &p.video, "snippet,status"))?;
    let title = now["snippet"]["title"].as_str().unwrap_or_default().to_string();
    let change = channel::merge(&now, &p)?;
    let answer = |text: String, title: String, privacy: Option<Privacy>| UpdateResult {
        text,
        video: p.video.clone(),
        url: channel::url(&p.video),
        changed: change.changed.iter().map(|c| c.to_string()).collect(),
        title,
        privacy: privacy.unwrap_or_default().into(),
        ..Default::default()
    };
    if change.changed.is_empty() {
        return Ok(answer(format!("Nothing to change on \"{title}\": it has those already."), title, change.was));
    }
    let fields = words::and(&change.changed);
    if change.needs_confirm() && !p.confirm {
        let why = if change.was == Some(Privacy::PRIVACY_PUBLIC) {
            "it's public"
        } else if change.becomes == Some(Privacy::PRIVACY_PUBLIC) {
            "this makes it public"
        } else {
            "this schedules it to go public"
        };
        bail!(
            "not changed: \"{title}\" ({}), and {why}, so youtube.update needs confirm=true, given only once the \
             channel's owner has said yes to changing its {fields}",
            channel::url(&p.video)
        );
    }
    let after = x.api(&p.account, |api| channel::update(api, &change))?;
    let title = after["snippet"]["title"].as_str().map(String::from).unwrap_or(title);
    let privacy = channel::privacy(&after).or(change.becomes);
    let text = format!(
        "Changed the {fields} of \"{title}\" ({}, {}).",
        channel::url(&p.video),
        privacy.map(Privacy::word).unwrap_or("privacy unknown")
    );
    Ok(answer(text, title, privacy))
}

fn thumbnail(x: &Ext, _: &Job, p: ThumbnailParams) -> anyhow::Result<ThumbnailResult> {
    check_video(&p.video)?;
    let path = x.file(&p.file, "file")?;
    let (bytes, kind) = upload::thumbnail_file(&path)?;
    x.api(&p.account, |api| {
        let now = channel::fetch(api, &p.video, "snippet,status")?;
        let title = now["snippet"]["title"].as_str().unwrap_or_default().to_string();
        if channel::privacy(&now) == Some(Privacy::PRIVACY_PUBLIC) && !p.confirm {
            bail!(
                "not changed: \"{title}\" ({}) is public, so youtube.thumbnail needs confirm=true, given only once the \
                 channel's owner has said yes to its new thumbnail",
                channel::url(&p.video)
            );
        }
        upload::thumbnail(api, &p.video, &bytes, kind)?;
        Ok(ThumbnailResult {
            text: format!("Set the thumbnail of \"{title}\" ({}) from {}.", channel::url(&p.video), name_of(&path)),
            url: channel::url(&p.video),
            video: p.video.clone(),
            ..Default::default()
        })
    })
}

/// youtube.chat's lines: one per message, at most `most`.
fn chat_lines(messages: &[ChatMessage], most: usize) -> String {
    let mut text = String::new();
    for m in messages.iter().take(most) {
        text.push_str(&format!("\n- {} {}: {} ({})", chat::clock(m), chat::who(m), chat::defang(&m.text), m.id));
    }
    if messages.len() > most {
        text.push_str(&format!("\n… and {} more in messages", messages.len() - most));
    }
    text
}

fn live(x: &Ext, _: &Job, p: LiveParams) -> anyhow::Result<LiveResult> {
    // a channel that can't live stream has no broadcasts: say so, and still say what the sign-in may do
    let listed = x.api(&p.account, |api| {
        let mut all = chat::broadcasts(api, "active")?;
        all.extend(chat::broadcasts(api, "upcoming")?);
        Ok(all)
    });
    let (broadcasts, streams) = match listed {
        Ok(all) => (all, true),
        Err(e) if chat::not_enabled(&e) => (vec![], false),
        Err(e) => return Err(e),
    };
    let scopes = x.scopes(&p.account)?;
    let can_moderate = chat::can_moderate(&scopes);
    let mut text = match broadcasts.len() {
        _ if !streams => "YouTube says this channel isn't enabled for live streaming (liveStreamingNotEnabled), so it \
                          has no broadcasts: turn it on in YouTube Studio (Go live, or youtube.com/features), which \
                          can take up to 24 hours the first time. Other channels' live chats can still be read."
            .to_string(),
        0 => "The channel has no live or upcoming broadcasts.".to_string(),
        n => format!("The channel's {n} live and upcoming broadcasts:"),
    };
    for b in &broadcasts {
        let when = b.starts_at.as_option().map(|t| words::day(&rfc3339(t), true)).unwrap_or_default();
        let chat = if b.chat_id.is_empty() { "no chat".to_string() } else { format!("chat {}", b.chat_id) };
        text.push_str(&format!("\n- {} ({}, {when}): {}, {chat}", b.title, b.status, b.url));
    }
    text.push_str(if can_moderate {
        "\nThe sign-in may send, remove and time out in chats the channel owns or moderates."
    } else {
        "\nThe sign-in can't moderate chat (it needs the youtube or youtube.force-ssl scope): sign in again with /youtube add."
    });
    Ok(LiveResult {
        text,
        broadcasts,
        can_stream: streams,
        scopes: scopes.split_whitespace().map(String::from).collect(),
        can_moderate,
        ..Default::default()
    })
}

fn chat_read(x: &Ext, _: &Job, p: ChatParams) -> anyhow::Result<ChatResult> {
    let (chat_id, page) = x.api(&p.account, |api| {
        let chat_id = chat::resolve(api, &p.video, &p.chat_id)?;
        let page = chat::page(api, &chat_id, p.page_token.trim(), "500")?;
        Ok((chat_id, page))
    })?;
    let mut text = match page.messages.len() {
        0 => "No new messages in the chat.".to_string(),
        n => format!("{n} chat messages, oldest first:"),
    };
    text.push_str(&chat_lines(&page.messages, 50));
    if let Some(at) = &page.offline_at {
        text.push_str(&format!("\nThe chat went offline {}.", words::day(at, true)));
    } else {
        text.push_str(&format!(
            "\nNext: youtube.chat with page_token={}, in {} s at the soonest.",
            page.next,
            (page.poll_ms as f64 / 1000.0).ceil()
        ));
    }
    Ok(ChatResult {
        text,
        messages: page.messages,
        page_token: page.next,
        poll_ms: page.poll_ms,
        chat_id,
        offline_at: page.offline_at.as_deref().and_then(timestamp).into(),
        ..Default::default()
    })
}

/// Waits `ms`, or less if the job is cancelled: whether it was.
fn wait(job: &Job, ms: u64) -> bool {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        if job.cancelled() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250).min(until - Instant::now()));
    }
    job.cancelled()
}

/// How long chat_watch waits between tries while the chat is down.
const RETRY_MS: u64 = 10_000;
/// The most rows in one batch's post: more go in another post.
const ROWS: usize = 50;

fn chat_watch(x: &Ext, job: &Job, p: ChatWatchParams) -> anyhow::Result<ChatWatchResult> {
    let every_ms = (p.every_seconds.clamp(5, 300) as u64) * 1000;
    let deadline = Instant::now() + Duration::from_secs(p.minutes.clamp(1, 720) as u64 * 60);
    let mention = chat::mention(&p.mention);
    let stop = |stopped: &str, chat_id: String| ChatWatchResult {
        text: format!("Stopped ({stopped}) before the chat of {} opened.", channel::url(&p.video)),
        stopped: stopped.to_string(),
        chat_id,
        ..Default::default()
    };
    // started before the stream: wait for its chat to open, as a drop does
    let mut said_waiting = false;
    let (mut chat_id, title) = loop {
        match x.api(&p.account, |api| chat::of_video(api, &p.video)) {
            Ok(found) => break found,
            Err(e) if chat::not_open(&e) => {
                if !said_waiting {
                    let line = format!(
                        "[{}] Waiting for the stream's chat to open ({}): trying every {} s.",
                        words::now_clock(),
                        channel::url(&p.video),
                        RETRY_MS / 1000
                    );
                    if let Err(e) = x.post(&p.thread, line, vec![]) {
                        bail!("couldn't post in thread {}: {e:#}", p.thread);
                    }
                    said_waiting = true;
                    job.progress(None, "waiting for the chat to open");
                }
                if Instant::now() >= deadline {
                    return Ok(stop("minutes", String::new()));
                }
                if wait(job, RETRY_MS) {
                    return Ok(stop("cancelled", String::new()));
                }
            }
            Err(e) => return Err(e),
        }
    };
    if said_waiting {
        let line = format!("[{}] The stream's chat is open: watching it.", words::now_clock());
        if let Err(e) = x.post(&p.thread, line, vec![]) {
            x.h.log(format!("couldn't post that the chat opened: {e:#}"));
        }
    }
    // the chat as it is now: what's in it already isn't posted
    let mut token = x.api(&p.account, |api| chat::page(api, &chat_id, "", "200"))?.next;
    let mut seen = std::collections::HashSet::new();
    let (mut batches, mut count) = (0i32, 0i32);
    let mut down: Option<String> = None;
    let mut posting_failed = 0;
    x.h.log(format!("watching the chat of {} ({chat_id}) for thread {}", p.video, p.thread));
    job.progress(None, "watching the chat");
    let stopped = loop {
        if Instant::now() >= deadline {
            break "minutes";
        }
        if wait(job, if down.is_some() { RETRY_MS } else { every_ms }) {
            break "cancelled";
        }
        // after a drop, the stream may come back with its chat again (or a new one): find it from the video
        if down.is_some() {
            match x.api(&p.account, |api| chat::of_video(api, &p.video)) {
                Ok((c, _)) => {
                    if c != chat_id {
                        token.clear();
                    }
                    chat_id = c;
                }
                Err(e) => {
                    x.h.log(format!("chat still down: {e:#}"));
                    continue;
                }
            }
        }
        let got = x.api(&p.account, |api| chat::page(api, &chat_id, &token, "2000"));
        let page = match got {
            Ok(page) if page.offline_at.is_none() => page,
            Ok(_) | Err(_) => {
                let why = match got {
                    Ok(_) => "the chat went offline".to_string(),
                    Err(e) if chat::ended(&e) => "the chat ended".to_string(),
                    Err(e) => format!("{e:#}"),
                };
                if down.is_none() {
                    let line = format!(
                        "[{}] YouTube chat dropped ({why}): trying again every {} s.",
                        words::now_clock(),
                        RETRY_MS / 1000
                    );
                    if let Err(e) = x.post(&p.thread, line, vec![]) {
                        x.h.log(format!("couldn't post the drop: {e:#}"));
                    }
                    down = Some(why);
                }
                job.progress(None, "the chat is down: trying again");
                continue;
            }
        };
        if let Some(why) = down.take() {
            let line = format!("[{}] YouTube chat is back (after: {why}).", words::now_clock());
            if let Err(e) = x.post(&p.thread, line, vec![]) {
                x.h.log(format!("couldn't post the return: {e:#}"));
            }
            job.progress(None, "watching the chat");
        }
        token = page.next;
        let fresh: Vec<ChatMessage> = page.messages.into_iter().filter(|m| seen.insert(m.id.clone())).collect();
        if seen.len() > 20_000 {
            seen.clear();
        }
        for (i, part) in fresh.chunks(ROWS).enumerate() {
            let mut text =
                format!("[{}] {} new in YouTube chat · {}", words::now_clock(), part.len(), chat::defang(&title));
            if fresh.len() > ROWS {
                text.push_str(&format!(" ({}/{})", i + 1, fresh.len().div_ceil(ROWS)));
            }
            if !mention.is_empty() {
                text.push_str(&format!(" {mention}"));
            }
            let rows = part
                .iter()
                .map(|m| (m.id.clone(), vec![json!(chat::clock(m)), json!(chat::who(m)), json!(chat::defang(&m.text))]))
                .collect();
            let table = blocks::table("chat", &["Time", "Who", "Message"], rows);
            match x.post(&p.thread, text, vec![table]) {
                Ok(_) => {
                    posting_failed = 0;
                    batches += 1;
                    count += part.len() as i32;
                }
                Err(e) => {
                    posting_failed += 1;
                    x.h.log(format!("couldn't post a batch: {e:#}"));
                    if posting_failed >= 3 || batches == 0 {
                        bail!("couldn't post the chat in thread {}: {e:#}", p.thread);
                    }
                }
            }
        }
        if (page.poll_ms as u64) > every_ms {
            // YouTube asks for a longer wait than every_seconds: keep to it
            if wait(job, page.poll_ms as u64 - every_ms) {
                break "cancelled";
            }
        }
        job.progress(None, format!("{count} messages in {batches} batches").as_str());
    };
    Ok(ChatWatchResult {
        text: format!(
            "Watched the chat of \"{title}\" ({}): {count} messages in {batches} batches posted; stopped: {stopped}.",
            channel::url(&p.video)
        ),
        batches,
        messages: count,
        stopped: stopped.to_string(),
        chat_id,
        ..Default::default()
    })
}

fn chat_send(x: &Ext, _: &Job, p: ChatSendParams) -> anyhow::Result<ChatSendResult> {
    if !p.confirm {
        bail!(
            "not sent: youtube.chat_send needs confirm=true, given only once the channel's owner has said yes to \
             sending \"{}\" in the chat of {}",
            p.text,
            channel::url(&p.video)
        );
    }
    let text = p.text.trim();
    if text.is_empty() || text.chars().count() > 200 {
        bail!("not sent: a chat message is 1 to 200 characters");
    }
    let (chat_id, title) = x.api(&p.account, |api| chat::of_video(api, &p.video))?;
    let id = x.api(&p.account, |api| chat::send(api, &chat_id, text))?;
    Ok(ChatSendResult {
        text: format!("Sent in the chat of \"{title}\": \"{text}\" ({id})."),
        message_id: id,
        chat_id,
        ..Default::default()
    })
}

fn chat_remove(x: &Ext, _: &Job, p: ChatRemoveParams) -> anyhow::Result<ChatRemoveResult> {
    if !p.confirm {
        bail!(
            "not removed: youtube.chat_remove needs confirm=true, given only once removing message {} was agreed \
             ({})",
            p.message_id,
            p.reason
        );
    }
    let reason = p.reason.trim();
    if reason.is_empty() {
        bail!("not removed: give the reason, for the log");
    }
    let m = x.api(&p.account, |api| {
        let chat_id = chat::resolve(api, &p.video, &p.chat_id)?;
        let page = chat::page(api, &chat_id, "", chat::MOST)?;
        match page.messages.into_iter().find(|m| m.id == p.message_id) {
            Some(m) => Ok(m),
            None => bail!(
                "not removed: message {} isn't among the latest messages of chat {chat_id}: check the video and the id",
                p.message_id
            ),
        }
    })?;
    x.api(&p.account, |api| chat::remove(api, &m.id))?;
    let line = chat::removed_line(&m, reason);
    let (logged, note) = match x.post(&p.thread, line, vec![]) {
        Ok(post) => (post, String::new()),
        Err(e) => (String::new(), format!(" It wasn't logged in thread {} ({e:#}): log it by hand.", p.thread)),
    };
    Ok(ChatRemoveResult {
        text: format!(
            "Removed {}'s message from {} (\"{}\"), reason: {reason}.{note}",
            chat::who(&m),
            chat::clock(&m),
            chat::defang(&m.text)
        ),
        message_id: m.id.clone(),
        message: m.into(),
        logged,
        ..Default::default()
    })
}

fn chat_timeout(x: &Ext, _: &Job, p: ChatTimeoutParams) -> anyhow::Result<ChatTimeoutResult> {
    if !p.confirm {
        bail!(
            "not timed out: youtube.chat_timeout needs confirm=true, given only once the owner has said yes to \
             keeping {} out of the chat for {} s",
            p.author_channel_id,
            p.seconds
        );
    }
    if !(10..=86_400).contains(&p.seconds) {
        bail!("not timed out: seconds is 10 to 86,400");
    }
    let chat_id = x.api(&p.account, |api| chat::resolve(api, &p.video, &p.chat_id))?;
    let ban = x.api(&p.account, |api| chat::timeout(api, &chat_id, &p.author_channel_id, p.seconds))?;
    let until = time::OffsetDateTime::now_utc().unix_timestamp() + p.seconds as i64;
    Ok(ChatTimeoutResult {
        text: format!("Timed {} out of the chat for {} s ({ban}).", p.author_channel_id, p.seconds),
        ban_id: ban,
        chat_id,
        until: buffa_types::google::protobuf::Timestamp { seconds: until, ..Default::default() }.into(),
        ..Default::default()
    })
}

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let x = Arc::new(Ext {
        h: ext.handle(),
        agent: google::agent(),
        ends: Endpoints::from_env(),
        cached: Mutex::new(None),
    });
    let signins = Arc::new(SignIns::default());

    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: VideosParams| videos(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: VideoParams| video(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: UploadParams| upload_video(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: UpdateParams| update(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ThumbnailParams| thumbnail(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: LiveParams| live(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ChatParams| chat_read(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ChatWatchParams| chat_watch(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ChatSendParams| chat_send(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ChatRemoveParams| chat_remove(&xx, job, p));
    let xx = x.clone();
    ext.typed_command(move |job: &Job, p: ChatTimeoutParams| chat_timeout(&xx, job, p));

    // `/youtube add` and `/youtube client` posted (an agent, an older app): the apps open the forms for them
    ext.typed_command(|_: &Job, _: AddParams| {
        Ok(AddResult {
            text: "A YouTube channel is signed in from the Pointman app, so its token never goes in a post: type \
                   `/youtube add` there and tap Sign in."
                .into(),
            ..Default::default()
        })
    });
    ext.typed_command(|_: &Job, _: ClientParams| {
        Ok(ClientResult {
            text: "The Google OAuth client is added in the Pointman app, so its secret never goes in a post: type \
                   `/youtube client` there and paste its client ID and secret."
                .into(),
            ..Default::default()
        })
    });

    // Sign in from the apps: Google's device flow
    let (xx, s) = (x.clone(), signins.clone());
    ext.on(move |p: CredentialsSignInRequest| {
        if p.form != "account" {
            bail!("the YouTube extension signs in channels only (form: account)");
        }
        if p.label.trim() == CLIENT_ITEM.trim_start_matches("youtube.") {
            bail!("{CLIENT_ITEM} is the OAuth client's place in the vault: call the channel something else");
        }
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        s.start(&xx.agent, &xx.ends, xx.client()?, session)
    });
    let (xx, s) = (x.clone(), signins.clone());
    ext.on(move |p: CredentialsWaitRequest| {
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        let value = s.wait(&xx.agent, &xx.ends, session, signin::WAIT)?;
        Ok(CredentialsWaitResponse { value, ..Default::default() })
    });
    let (xx, s) = (x.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        let value = s.code(&xx.agent, &xx.ends, session, &p.code, Duration::from_secs(150))?;
        Ok(CredentialsCodeResponse { value, ..Default::default() })
    });

    ext.run()
}
