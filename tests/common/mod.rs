//! A fake YouTube: Google's OAuth endpoints (refresh tokens, the device flow) and the parts of the Data API v3 the
//! extension calls, on a local port, over plain HTTP/1.1. It keeps every request it was sent, and behaves as YouTube
//! does where it matters to the tests: videos.update replaces every mutable property of each part it's sent, a
//! resumable upload keeps whole 256 KiB pieces of a chunk whose connection dropped, and a request without a token it
//! gave is a 401.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Map, Value};

pub const CLIENT_ID: &str = "123-fake.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "GOCSPX-fakeclientsecret01";
pub const REFRESH: &str = "1//0gFAKErefreshTOKENfor-the-tests";
pub const SCOPE: &str = "https://www.googleapis.com/auth/youtube";
pub const USER_CODE: &str = "WDJB-MJHT";
pub const QUANTUM: usize = 256 * 1024;

/// One request it was sent.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    /// How many bytes of its body it read (all of them, but for a dropped one).
    pub read: usize,
    pub json: Value,
}

pub struct Upload {
    pub size: usize,
    pub data: Vec<u8>,
    pub details: Value,
    /// The video it made, once it has every byte.
    pub video: Option<String>,
}

#[derive(Default)]
pub struct State {
    pub requests: Vec<Req>,
    pub videos: HashMap<String, Value>,
    /// The uploads playlist, newest first.
    pub order: Vec<String>,
    pub uploads: HashMap<String, Upload>,
    /// Drop the connection once, partway through the chunk that reaches this byte of an upload.
    pub drop_at: Option<usize>,
    pub thumbnails: HashMap<String, (String, usize)>,
    pub playlist_items: Vec<(String, String)>,
    pub tokens: Vec<String>,
    /// Tokens it says 401 to once, as if they had run out.
    pub expire: Vec<String>,
    /// Device-flow polls answered "authorization_pending" before the person says yes.
    pub pending_polls: u32,
    pub made: u32,
}

#[derive(Clone)]
pub struct Fake {
    pub url: String,
    state: Arc<Mutex<State>>,
}

impl Fake {
    pub fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let fake = Fake { url, state: Arc::new(Mutex::new(State { pending_polls: 1, ..Default::default() })) };
        let f = fake.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let f = f.clone();
                std::thread::spawn(move || f.serve(stream));
            }
        });
        fake
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn requests(&self) -> Vec<Req> {
        self.state().requests.clone()
    }

    /// The requests to the Data API (not OAuth's).
    pub fn api_requests(&self) -> Vec<Req> {
        self.requests().into_iter().filter(|r| r.path != "/token" && r.path != "/device/code").collect()
    }

    pub fn video(&self, id: &str) -> Value {
        self.state().videos.get(id).cloned().unwrap_or(Value::Null)
    }

    /// A video on the channel, newest so far.
    pub fn add_video(&self, id: &str, title: &str, privacy: &str, views: u64) {
        let v = json!({
            "kind": "youtube#video",
            "id": id,
            "snippet": {
                "publishedAt": "2026-10-01T10:00:00Z", "channelId": "UCfake", "title": title,
                "description": format!("About {title}.\n\nLinks:\n- https://example.com"),
                "thumbnails": {"default": {"url": "https://i.ytimg.com/vi/x/default.jpg"}},
                "channelTitle": "Fake channel", "tags": ["ai", "claude code"], "categoryId": "28",
                "liveBroadcastContent": "none", "defaultLanguage": "en", "defaultAudioLanguage": "en",
                "localized": {"title": title, "description": "…"}
            },
            "status": {
                "uploadStatus": "processed", "privacyStatus": privacy, "license": "youtube", "embeddable": true,
                "publicStatsViewable": true, "madeForKids": false, "selfDeclaredMadeForKids": false
            },
            "statistics": {"viewCount": views.to_string(), "likeCount": "3", "commentCount": "1"},
            "contentDetails": {"duration": "PT12M3S"}
        });
        let mut s = self.state();
        s.videos.insert(id.into(), v);
        s.order.insert(0, id.into());
    }

    fn serve(&self, mut stream: TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut words = line.split_whitespace();
        let method = words.next().unwrap_or_default().to_string();
        let target = words.next().unwrap_or_default().to_string();
        let mut headers = HashMap::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), form(q)),
            None => (target.clone(), HashMap::new()),
        };
        let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);

        // a chunk whose connection drops: read part of it, keep whole 256 KiB pieces, and hang up
        if method == "PUT" && path.starts_with("/upload/session/") {
            if let Some((from, _)) = content_range(&headers) {
                let drop = self.state().drop_at.filter(|at| (from..from + length).contains(at));
                if let Some(at) = drop {
                    let mut part = vec![0u8; at - from];
                    let read = reader.read_exact(&mut part).map(|_| part.len()).unwrap_or(0);
                    let mut s = self.state();
                    s.drop_at = None;
                    s.requests.push(Req { method, path: path.clone(), query, headers, read, json: Value::Null });
                    if let Some(up) = s.uploads.get_mut(path.trim_start_matches("/upload/session/")) {
                        if up.data.len() == from {
                            up.data.extend_from_slice(&part[..read]);
                            let keep = up.data.len() / QUANTUM * QUANTUM;
                            up.data.truncate(keep);
                        }
                    }
                    drop_conn(stream);
                    return;
                }
            }
        }
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let as_json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        self.state().requests.push(Req {
            method: method.clone(),
            path: path.clone(),
            query: query.clone(),
            headers: headers.clone(),
            read: body.len(),
            json: as_json.clone(),
        });
        let (status, extra, out) = self.answer(&method, &path, &query, &headers, &body, &as_json);
        let text = if out.is_null() { String::new() } else { out.to_string() };
        let mut head = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n",
            reason(status),
            text.len()
        );
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.flush();
    }

    fn authorized(&self, headers: &HashMap<String, String>) -> bool {
        let Some(token) = headers.get("authorization").and_then(|a| a.strip_prefix("Bearer ")) else { return false };
        let mut s = self.state();
        if let Some(i) = s.expire.iter().position(|t| t == token) {
            s.expire.remove(i);
            return false;
        }
        s.tokens.iter().any(|t| t == token)
    }

    fn answer(
        &self,
        method: &str,
        path: &str,
        query: &HashMap<String, String>,
        headers: &HashMap<String, String>,
        body: &[u8],
        body_json: &Value,
    ) -> (u16, Vec<(String, String)>, Value) {
        let error = |status: u16, reason: &str, message: &str| {
            (status, vec![], json!({"error": {"code": status, "message": message, "errors": [{"reason": reason}]}}))
        };
        if path == "/token" || path == "/device/code" {
            return self.oauth(path, &form(&String::from_utf8_lossy(body)));
        }
        if !self.authorized(headers) {
            return error(401, "authError", "Invalid Credentials");
        }
        let mut s = self.state();
        match (method, path) {
            ("GET", "/youtube/v3/channels") => {
                let n = s.order.len();
                (
                    200,
                    vec![],
                    json!({"items": [{"id": "UCfake",
                    "contentDetails": {"relatedPlaylists": {"uploads": "UUfake"}},
                    "statistics": {"videoCount": n.to_string()}}]}),
                )
            }
            ("GET", "/youtube/v3/playlistItems") => {
                let size: usize = query.get("maxResults").and_then(|m| m.parse().ok()).unwrap_or(5);
                let from: usize =
                    query.get("pageToken").and_then(|p| p.strip_prefix('p')).and_then(|p| p.parse().ok()).unwrap_or(0);
                let ids: Vec<&String> = s.order.iter().skip(from).take(size).collect();
                let items: Vec<Value> = ids.iter().map(|id| json!({"contentDetails": {"videoId": id}})).collect();
                let mut out = json!({"items": items, "pageInfo": {"totalResults": s.order.len()}});
                if from + size < s.order.len() {
                    out["nextPageToken"] = json!(format!("p{}", from + size));
                }
                (200, vec![], out)
            }
            ("GET", "/youtube/v3/videos") => {
                let parts: Vec<&str> = query.get("part").map(|p| p.split(',').collect()).unwrap_or_default();
                // in another order than asked, as YouTube may give them
                let mut items: Vec<Value> = query
                    .get("id")
                    .map(|ids| ids.split(',').filter_map(|id| s.videos.get(id)).cloned().collect())
                    .unwrap_or_default();
                items.reverse();
                let items: Vec<Value> = items
                    .into_iter()
                    .map(|v| {
                        let mut o = Map::new();
                        o.insert("id".into(), v["id"].clone());
                        for p in &parts {
                            o.insert(p.to_string(), v[*p].clone());
                        }
                        Value::Object(o)
                    })
                    .collect();
                (200, vec![], json!({"items": items}))
            }
            ("PUT", "/youtube/v3/videos") => {
                let id = body_json["id"].as_str().unwrap_or_default().to_string();
                let Some(mut v) = s.videos.get(&id).cloned() else { return error(404, "videoNotFound", "no video") };
                for part in query.get("part").map(String::as_str).unwrap_or_default().split(',') {
                    let Some(sent) = body_json[part].as_object() else {
                        return error(400, "invalidPart", "a part named isn't in the body");
                    };
                    if part == "snippet" && (sent.get("title").is_none() || sent.get("categoryId").is_none()) {
                        return error(400, "invalidCategoryId", "snippet needs title and categoryId");
                    }
                    // every mutable property of the part is replaced: what isn't sent is gone; read-only ones stay
                    let read_only: &[&str] = match part {
                        "snippet" => &[
                            "publishedAt",
                            "channelId",
                            "channelTitle",
                            "thumbnails",
                            "liveBroadcastContent",
                            "localized",
                        ],
                        "status" => &["uploadStatus", "failureReason", "rejectionReason", "madeForKids"],
                        _ => &[],
                    };
                    let mut now: Map<String, Value> = sent
                        .iter()
                        .filter(|(k, _)| !read_only.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for k in read_only {
                        if let Some(old) = v[part].get(*k) {
                            now.insert(k.to_string(), old.clone());
                        }
                    }
                    v[part] = Value::Object(now);
                }
                s.videos.insert(id, v.clone());
                (200, vec![], v)
            }
            ("POST", "/upload/youtube/v3/videos") => {
                if query.get("uploadType").map(String::as_str) != Some("resumable") {
                    return error(400, "badRequest", "not resumable");
                }
                let size: usize = headers.get("x-upload-content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                s.made += 1;
                let session = format!("s{}", s.made);
                s.uploads
                    .insert(session.clone(), Upload { size, data: vec![], details: body_json.clone(), video: None });
                (200, vec![("Location".into(), format!("{}/upload/session/{session}", self.url))], Value::Null)
            }
            ("PUT", p) if p.starts_with("/upload/session/") => {
                let session = p.trim_start_matches("/upload/session/").to_string();
                let range = content_range(headers);
                let id = format!("vidUP{:06}", s.made);
                let details = {
                    let Some(up) = s.uploads.get_mut(&session) else {
                        return error(404, "notFound", "no upload session");
                    };
                    if let Some(made) = &up.video {
                        let made = made.clone();
                        return (200, vec![], s.videos[&made].clone());
                    }
                    if let Some((from, _)) = range {
                        if from == up.data.len() {
                            up.data.extend_from_slice(body);
                        } else if !body.is_empty() {
                            return error(400, "badRange", "a chunk that doesn't follow what's there");
                        }
                    }
                    if up.data.len() < up.size {
                        let extra = if up.data.is_empty() {
                            vec![]
                        } else {
                            vec![("Range".to_string(), format!("bytes=0-{}", up.data.len() - 1))]
                        };
                        return (308, extra, Value::Null);
                    }
                    up.video = Some(id.clone());
                    up.details.clone()
                };
                let mut snippet = details["snippet"].clone();
                snippet["publishedAt"] = json!("2026-10-26T09:00:00Z");
                snippet["channelId"] = json!("UCfake");
                let mut status = details["status"].clone();
                status["uploadStatus"] = json!("uploaded");
                let v = json!({"kind": "youtube#video", "id": id, "snippet": snippet, "status": status,
                               "statistics": {"viewCount": "0"}, "contentDetails": {"duration": "PT0S"}});
                s.videos.insert(id.clone(), v.clone());
                s.order.insert(0, id);
                (201, vec![], v)
            }
            ("POST", "/upload/youtube/v3/thumbnails/set") => {
                let video = query.get("videoId").cloned().unwrap_or_default();
                if !s.videos.contains_key(&video) {
                    return error(404, "videoNotFound", "no video");
                }
                let kind = headers.get("content-type").cloned().unwrap_or_default();
                s.thumbnails.insert(video, (kind, body.len()));
                (200, vec![], json!({"items": [{"default": {"url": "https://i.ytimg.com/vi/x/default.jpg"}}]}))
            }
            ("POST", "/youtube/v3/playlistItems") => {
                let sn = &body_json["snippet"];
                let pl = sn["playlistId"].as_str().unwrap_or_default().to_string();
                let video = sn["resourceId"]["videoId"].as_str().unwrap_or_default().to_string();
                s.playlist_items.push((pl, video));
                (200, vec![], json!({"id": "pli1", "snippet": sn}))
            }
            _ => error(404, "notFound", "no such endpoint in the fake"),
        }
    }

    fn oauth(&self, path: &str, f: &HashMap<String, String>) -> (u16, Vec<(String, String)>, Value) {
        let oops = |status: u16, error: &str| (status, vec![], json!({"error": error, "error_description": error}));
        let client_ok = f.get("client_id").map(String::as_str) == Some(CLIENT_ID);
        if !client_ok {
            return oops(401, "invalid_client");
        }
        let mut s = self.state();
        if path == "/device/code" {
            if f.get("scope").map(String::as_str) != Some(SCOPE) {
                return oops(400, "invalid_scope");
            }
            return (
                200,
                vec![],
                json!({"device_code": "dev-code-1", "user_code": USER_CODE,
                "verification_url": "https://www.google.com/device", "expires_in": 1800, "interval": 1}),
            );
        }
        if f.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET) {
            return oops(401, "invalid_client");
        }
        s.made += 1;
        let token = format!("ya29.fake-access-{}", s.made);
        match f.get("grant_type").map(String::as_str) {
            Some("refresh_token") => {
                if f.get("refresh_token").map(String::as_str) != Some(REFRESH) {
                    return oops(400, "invalid_grant");
                }
                s.tokens.push(token.clone());
                (
                    200,
                    vec![],
                    json!({"access_token": token, "expires_in": 3599, "scope": SCOPE, "token_type": "Bearer"}),
                )
            }
            Some("urn:ietf:params:oauth:grant-type:device_code") => {
                if f.get("device_code").map(String::as_str) != Some("dev-code-1") {
                    return oops(400, "invalid_grant");
                }
                if s.pending_polls > 0 {
                    s.pending_polls -= 1;
                    return oops(428, "authorization_pending");
                }
                s.tokens.push(token.clone());
                (
                    200,
                    vec![],
                    json!({"access_token": token, "expires_in": 3599, "refresh_token": REFRESH,
                                     "scope": SCOPE, "token_type": "Bearer"}),
                )
            }
            _ => oops(400, "unsupported_grant_type"),
        }
    }
}

fn drop_conn(stream: TcpStream) {
    let _ = stream.shutdown(Shutdown::Both);
}

/// `Content-Range: bytes <from>-<to>/<size>`: from and to; `bytes */<size>` has none.
pub fn content_range(headers: &HashMap<String, String>) -> Option<(usize, usize)> {
    let r = headers.get("content-range")?.strip_prefix("bytes ")?;
    let (span, _) = r.split_once('/')?;
    let (a, b) = span.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        308 => "Resume Incomplete",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        428 => "Precondition Required",
        _ => "Status",
    }
}

/// A query string or form body.
pub fn form(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bytes that look like nothing in particular: a video's stand-in.
pub fn bytes(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}
