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
use serde_json::{json, Value};
use youtube::google::{self, Api, Endpoints, OAuthClient};
use youtube::params::{self, check_publish_at, check_video, Privacy, ThumbnailParams, UpdateParams, UploadParams};
use youtube::signin::{self, SignIns};
use youtube::{channel, upload, words, CLIENT_ITEM};

struct Cached {
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

    /// The vault item of the channel's sign-in to use: the setting's, else the first by label.
    fn account(&self) -> anyhow::Result<String> {
        let channels: Vec<String> = self.h.secrets("youtube.")?.into_iter().filter(|n| n != CLIENT_ITEM).collect();
        let want = self.h.settings()["account"].as_str().unwrap_or_default().trim().to_string();
        if !want.is_empty() {
            let name = format!("youtube.{want}");
            if channels.contains(&name) {
                return Ok(name);
            }
            let have = if channels.is_empty() { "none".to_string() } else { channels.join(", ") };
            bail!(
                "no YouTube sign-in {name} on {} (the account setting); it has {have}: sign it in with /youtube add",
                self.machine()
            );
        }
        channels.into_iter().min().ok_or_else(|| {
            anyhow!(
                "no YouTube channel is signed in on {}: add the Google OAuth client (/youtube client) if it isn't there, \
                 then type /youtube add in the app and tap Sign in",
                self.machine()
            )
        })
    }

    fn client(&self) -> anyhow::Result<OAuthClient> {
        let value = self.h.secret(CLIENT_ITEM).map_err(|_| {
            anyhow!(
                "no Google OAuth client on {}: type /youtube client in the app and paste a TVs and Limited Input \
                 client's ID and secret",
                self.machine()
            )
        })?;
        OAuthClient::parse(&value).ok_or_else(|| {
            anyhow!(
                "{CLIENT_ITEM} isn't a client ID and secret (…apps.googleusercontent.com, a space, then the secret)"
            )
        })
    }

    /// An access token for the channel, made again when it's within a minute of running out, or `fresh`.
    fn token(&self, fresh: bool) -> anyhow::Result<String> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cached.as_ref().filter(|c| !fresh && c.until > Instant::now() + Duration::from_secs(60)) {
            return Ok(c.token.clone());
        }
        let name = self.account()?;
        let value = self.h.secret(&name)?;
        let refresh = Regex::new(r"1//[A-Za-z0-9_-]+")
            .expect("compiles")
            .find(&value)
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                anyhow!("{name} isn't a Google refresh token: sign the channel in again with /youtube add")
            })?;
        let (token, lasts) = google::access_token(&self.agent, &self.ends, &self.client()?, &refresh)?;
        *cached = Some(Cached { token: token.clone(), until: Instant::now() + lasts });
        Ok(token)
    }

    fn api<T>(&self, f: impl FnOnce(&Api) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let token = |fresh: bool| self.token(fresh);
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

fn videos(x: &Ext, job: &Job) -> anyhow::Result<Value> {
    let p: params::VideosParams = params::read(&job.params)?;
    let (rows, next, total) = x.api(|api| channel::videos(api, p.page_size.unwrap_or(20), &p.page))?;
    let mut text = if rows.is_empty() {
        "No videos on the channel.".to_string()
    } else {
        format!("{} of the channel's {} videos, newest first:", rows.len(), words::count(total))
    };
    for r in &rows {
        let when = match r["publish_at"].as_str() {
            Some(at) => format!("goes public {}", words::day(at, true)),
            None => words::day(r["published_at"].as_str().unwrap_or_default(), false),
        };
        text.push_str(&format!(
            "\n- {} ({}, {} views, {when}): {}",
            r["title"].as_str().unwrap_or_default(),
            r["privacy"].as_str().unwrap_or_default(),
            words::count(r["views"].as_u64().unwrap_or(0)),
            r["id"].as_str().unwrap_or_default(),
        ));
    }
    if !next.is_empty() {
        text.push_str(&format!("\nMore: youtube.videos with page={next}"));
    }
    Ok(json!({"text": text, "videos": rows, "next_page": next, "total": total}))
}

fn video(x: &Ext, job: &Job) -> anyhow::Result<Value> {
    let p: params::VideoParams = params::read(&job.params)?;
    check_video(&p.video)?;
    let v = x.api(|api| channel::fetch(api, &p.video, "snippet,status,statistics,contentDetails"))?;
    let (s, st) = (&v["snippet"], &v["status"]);
    let title = s["title"].as_str().unwrap_or_default();
    let privacy = st["privacyStatus"].as_str().unwrap_or_default();
    let views = v["statistics"]["viewCount"].as_str().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    let mut out = json!({
        "video": p.video,
        "url": channel::url(&p.video),
        "title": title,
        "description": s["description"].as_str().unwrap_or_default(),
        "tags": s["tags"].as_array().cloned().unwrap_or_default(),
        "category": channel::category_word(s["categoryId"].as_str().unwrap_or_default()),
        "privacy": privacy,
        "published_at": s["publishedAt"],
        "views": views,
        "duration": v["contentDetails"]["duration"].as_str().unwrap_or_default(),
    });
    if let Some(at) = st["publishAt"].as_str() {
        out["publish_at"] = json!(at);
    }
    if let Some(k) = st["selfDeclaredMadeForKids"].as_bool() {
        out["made_for_kids"] = json!(k);
    }
    let schedule =
        st["publishAt"].as_str().map(|at| format!(", goes public {}", words::day(at, true))).unwrap_or_default();
    out["text"] = json!(format!(
        "\"{title}\" ({privacy}{schedule}, {} views): {}, {} tags, {} characters of description.",
        words::count(views),
        channel::url(&p.video),
        out["tags"].as_array().map_or(0, Vec::len),
        out["description"].as_str().map_or(0, |d| d.chars().count()),
    ));
    Ok(out)
}

fn upload_video(x: &Ext, job: &Job) -> anyhow::Result<Value> {
    let p: UploadParams = params::read(&job.params)?;
    let privacy = p.privacy.unwrap_or(Privacy::Private);
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
    let publish_at = p.publish_at.as_deref().map(check_publish_at).transpose()?;
    let category = p.category.map(|c| c.id()).unwrap_or_else(|| "22".to_string());
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
    let chunk = x.h.settings()["chunk_mib"].as_u64().unwrap_or(8).clamp(1, 256) << 20;
    let size = std::fs::metadata(&path)?.len();
    let started = Instant::now();

    let made = x.api(|api| upload::video(job, api, &path, &details, chunk, resume))?;
    let id = made["id"]
        .as_str()
        .filter(|i| !i.is_empty())
        .ok_or_else(|| anyhow!("YouTube didn't say the new video's id"))?;
    let mut text = format!(
        "Uploaded {} ({}, in {} s) as \"{}\": {}, {}",
        name_of(&path),
        words::size(size),
        started.elapsed().as_secs(),
        p.title,
        channel::url(id),
        privacy.word()
    );
    if let Some(at) = &publish_at {
        text.push_str(&format!(", going public {}", words::day(at, true)));
    }
    text.push('.');
    let mut thumbnail = false;
    if let Some((bytes, kind)) = thumb {
        job.progress(1.0, "setting the thumbnail");
        match x.api(|api| upload::thumbnail(api, id, &bytes, kind)) {
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
        match x.api(|api| channel::add_to_playlist(api, &p.playlist, id)) {
            Ok(()) => {
                playlist = p.playlist.clone();
                text.push_str(&format!(" Added to playlist {}.", p.playlist));
            }
            Err(e) => text.push_str(&format!(" It wasn't added to playlist {} ({e:#}).", p.playlist)),
        }
    }
    text.push_str(" YouTube processes it before it plays in full.");
    let mut out = json!({
        "text": text, "video": id, "url": channel::url(id), "privacy": privacy.word(), "bytes": size,
        "thumbnail": thumbnail, "playlist": playlist,
    });
    if let Some(at) = publish_at {
        out["publish_at"] = json!(at);
    }
    Ok(out)
}

fn update(x: &Ext, job: &Job) -> anyhow::Result<Value> {
    let p: UpdateParams = params::read(&job.params)?;
    p.check()?;
    let now = x.api(|api| channel::fetch(api, &p.video, "snippet,status"))?;
    let title = now["snippet"]["title"].as_str().unwrap_or_default().to_string();
    let change = channel::merge(&now, &p)?;
    if change.changed.is_empty() {
        return Ok(json!({
            "text": format!("Nothing to change on \"{title}\": it has those already."),
            "video": p.video, "url": channel::url(&p.video), "changed": [], "title": title,
            "privacy": change.was.map(Privacy::word),
        }));
    }
    let fields = words::and(&change.changed);
    if change.needs_confirm() && !p.confirm {
        let why = if change.was == Some(Privacy::Public) {
            "it's public"
        } else if change.becomes == Some(Privacy::Public) {
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
    let after = x.api(|api| channel::update(api, &change))?;
    let title = after["snippet"]["title"].as_str().map(String::from).unwrap_or(title);
    let privacy = channel::privacy(&after).or(change.becomes);
    let text = format!(
        "Changed the {fields} of \"{title}\" ({}, {}).",
        channel::url(&p.video),
        privacy.map(Privacy::word).unwrap_or("privacy unknown")
    );
    Ok(json!({
        "text": text, "video": p.video, "url": channel::url(&p.video), "changed": change.changed, "title": title,
        "privacy": privacy.map(Privacy::word),
    }))
}

fn thumbnail(x: &Ext, job: &Job) -> anyhow::Result<Value> {
    let p: ThumbnailParams = params::read(&job.params)?;
    check_video(&p.video)?;
    let path = x.file(&p.file, "file")?;
    let (bytes, kind) = upload::thumbnail_file(&path)?;
    x.api(|api| {
        let now = channel::fetch(api, &p.video, "snippet,status")?;
        let title = now["snippet"]["title"].as_str().unwrap_or_default().to_string();
        if channel::privacy(&now) == Some(Privacy::Public) && !p.confirm {
            bail!(
                "not changed: \"{title}\" ({}) is public, so youtube.thumbnail needs confirm=true, given only once the \
                 channel's owner has said yes to its new thumbnail",
                channel::url(&p.video)
            );
        }
        upload::thumbnail(api, &p.video, &bytes, kind)?;
        Ok(json!({
            "text": format!("Set the thumbnail of \"{title}\" ({}) from {}.", channel::url(&p.video), name_of(&path)),
            "video": p.video, "url": channel::url(&p.video),
        }))
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

    for (kind, f) in [
        ("youtube.videos", videos as fn(&Ext, &Job) -> anyhow::Result<Value>),
        ("youtube.video", video),
        ("youtube.upload", upload_video),
        ("youtube.update", update),
        ("youtube.thumbnail", thumbnail),
    ] {
        let x = x.clone();
        ext.command(kind, move |job: &Job| f(&x, job));
    }

    // `/youtube add` and `/youtube client` posted (an agent, an older app): the apps open the forms for them
    ext.command("youtube.add", |_: &Job| {
        Ok(json!({"text": "A YouTube channel is signed in from the Pointman app, so its token never goes in a post: \
                           type `/youtube add` there and tap Sign in."}))
    });
    ext.command("youtube.client", |_: &Job| {
        Ok(json!({"text": "The Google OAuth client is added in the Pointman app, so its secret never goes in a post: \
                           type `/youtube client` there and paste its client ID and secret."}))
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
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        s.start(&xx.agent, &xx.ends, xx.client()?, session)
    });
    let (xx, s) = (x.clone(), signins.clone());
    ext.on(move |p: CredentialsWaitRequest| {
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        let value = s.wait(&xx.agent, &xx.ends, session, signin::WAIT)?;
        Ok(CredentialsWaitResponse { value, ..Default::default() })
    });
    let (xx, s) = (x.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        let value = s.code(&xx.agent, &xx.ends, session, &p.code, Duration::from_secs(150))?;
        Ok(CredentialsCodeResponse { value, ..Default::default() })
    });

    ext.run()
}
