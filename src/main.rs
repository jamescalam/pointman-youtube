//! The extension's process: the node starts it (`bin/youtube`, from the extension's folder) and talks to it over stdin
//! and stdout. It lists the channel's videos, uploads new ones and changes their details, and signs channels in from
//! the apps. Nothing reaches the channel without a sign-in in the vault, and nothing is uploaded, and no public video
//! changed, without `confirm`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use pointman_extension::protocol::{
    CredentialsCodeRequest, CredentialsCodeResponse, CredentialsSignInRequest, CredentialsWaitRequest,
    CredentialsWaitResponse,
};
use pointman_extension::{Extension, Handle, Job};
use regex::Regex;
use serde_json::json;
use youtube::google::{self, Api, Endpoints, OAuthClient};
use youtube::params::{check_video, known, rfc3339, timestamp};
use youtube::signin::{self, SignIns};
use youtube::v1::{
    AddParams, AddResult, Category, ClientParams, ClientResult, Privacy, Settings, ThumbnailParams, ThumbnailResult,
    UpdateParams, UpdateResult, UploadParams, UploadResult, VideoParams, VideoResult, VideosParams, VideosResult,
};
use youtube::{channel, upload, words, CLIENT_ITEM};

struct Cached {
    name: String,
    token: String,
    until: Instant,
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
        let (token, lasts) = google::access_token(&self.agent, &self.ends, &self.client()?, &refresh)?;
        *cached = Some(Cached { name, token: token.clone(), until: Instant::now() + lasts });
        Ok(token)
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
