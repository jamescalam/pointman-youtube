//! Google's HTTP APIs: OAuth (a refresh token into an access token) and the YouTube Data API v3, with a token that's
//! made again when it runs out. Errors say what Google said, never a token.

use std::io::Read;
use std::time::Duration;

use anyhow::anyhow;
use regex::Regex;
use serde_json::Value;

/// Where the calls go: Google's own, unless a test's stand-in is named in the environment (the node never passes its
/// own environment on, so only a test can).
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// The Data API: `{api}/youtube/v3/…` and `{api}/upload/youtube/v3/…`.
    pub api: String,
    /// OAuth: `{oauth}/token` and `{oauth}/device/code`.
    pub oauth: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let get = |k: &str, default: &str| {
            std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
        };
        Endpoints {
            api: get("YOUTUBE_API_BASE", "https://www.googleapis.com").trim_end_matches('/').to_string(),
            oauth: get("YOUTUBE_OAUTH_BASE", "https://oauth2.googleapis.com").trim_end_matches('/').to_string(),
        }
    }
}

/// The scope every sign-in asks for: manage the channel's videos (allowed for the device flow, and what videos.insert
/// takes).
pub const SCOPE: &str = "https://www.googleapis.com/auth/youtube";

/// The Google OAuth client the sign-ins go through (the vault's `youtube.client`).
#[derive(Clone)]
pub struct OAuthClient {
    pub id: String,
    pub secret: String,
}

impl std::fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClient").field("id", &self.id).finish_non_exhaustive()
    }
}

impl OAuthClient {
    /// The vault item's value: the client id, then its secret (a space or a colon between).
    pub fn parse(value: &str) -> Option<Self> {
        let re = Regex::new(r"([0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com)[\s:]+([A-Za-z0-9_-]{16,128})")
            .expect("compiles");
        let m = re.captures(value)?;
        Some(OAuthClient { id: m[1].to_string(), secret: m[2].to_string() })
    }
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        // a resumable upload's "308 Resume Incomplete" is an answer, not a redirect
        .redirects(0)
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(300))
        .user_agent(concat!("pointman-youtube/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// What went wrong with a call.
#[derive(Debug)]
pub enum Failure {
    /// Google answered with an error: its status, the reason it gave (`quotaExceeded`, `invalid_grant`) and its words.
    Said { status: u16, reason: String, message: String },
    /// No answer: the connection dropped or timed out.
    Lost(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Said { status, reason, message } => {
                let hint = match reason.as_str() {
                    "quotaExceeded" | "dailyLimitExceeded" => {
                        " (this OAuth client's daily YouTube quota is used up: it resets at midnight Pacific time)"
                    }
                    "uploadLimitExceeded" => " (the channel has hit YouTube's upload limit for now)",
                    "youtubeSignupRequired" => " (the signed-in Google account has no YouTube channel)",
                    _ => "",
                };
                write!(f, "YouTube said {status}")?;
                if !reason.is_empty() {
                    write!(f, " {reason}")?;
                }
                if !message.is_empty() {
                    write!(f, ": {message}")?;
                }
                f.write_str(hint)
            }
            Failure::Lost(why) => write!(f, "no answer from YouTube: {why}"),
        }
    }
}

impl std::error::Error for Failure {}

/// The reason and words of Google's error body, as the Data API (`{error: {code, message, errors: [{reason}]}}`) or
/// OAuth (`{error, error_description}`) says them.
pub fn said(status: u16, body: &str) -> Failure {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (reason, message) = match &v["error"] {
        Value::Object(e) => (
            e.get("errors")
                .and_then(|x| x[0]["reason"].as_str())
                .or_else(|| e.get("status").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string(),
            e.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
        Value::String(e) => (e.clone(), v["error_description"].as_str().unwrap_or_default().to_string()),
        _ => (String::new(), body.chars().take(200).collect()),
    };
    Failure::Said { status, reason, message }
}

/// A response's body as text, at most 4 MiB.
pub fn body(r: ureq::Response) -> String {
    let mut s = String::new();
    let _ = r.into_reader().take(4 << 20).read_to_string(&mut s);
    s
}

/// The answer to a request, any status below 400 as Ok (a 308 too), or what went wrong.
pub fn send(r: Result<ureq::Response, ureq::Error>) -> Result<ureq::Response, Failure> {
    match r {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(status, r)) => Err(said(status, &body(r))),
        Err(ureq::Error::Transport(t)) => Err(Failure::Lost(t.to_string())),
    }
}

pub fn json(r: ureq::Response) -> Result<Value, Failure> {
    let status = r.status();
    let text = body(r);
    serde_json::from_str(&text).map_err(|_| Failure::Said {
        status,
        reason: String::new(),
        message: format!("not JSON: {}", text.chars().take(200).collect::<String>()),
    })
}

/// An access token for a refresh token: the token and how long it lasts.
pub fn access_token(
    agent: &ureq::Agent,
    ends: &Endpoints,
    client: &OAuthClient,
    refresh: &str,
) -> anyhow::Result<(String, Duration)> {
    let got = send(agent.post(&format!("{}/token", ends.oauth)).send_form(&[
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.as_str()),
        ("refresh_token", refresh),
        ("grant_type", "refresh_token"),
    ]));
    let v = match got {
        Ok(r) => json(r)?,
        Err(Failure::Said { reason, .. }) if reason == "invalid_grant" => {
            return Err(anyhow!(
                "Google no longer takes this sign-in (invalid_grant: revoked, unused for 6 months, or 7 days old while \
                 the OAuth app is in testing): sign the channel in again with /youtube add"
            ))
        }
        Err(Failure::Said { reason, .. }) if reason == "invalid_client" || reason == "unauthorized_client" => {
            return Err(anyhow!(
                "Google refused the OAuth client ({reason}): check youtube.client is a TVs and Limited Input client, \
                 and that this sign-in was made with it"
            ))
        }
        Err(e) => return Err(e.into()),
    };
    let token =
        v["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("Google gave no access token"))?;
    Ok((token.to_string(), Duration::from_secs(v["expires_in"].as_u64().unwrap_or(3600))))
}

/// Calls to the Data API with a token from `token` (true: make a new one, the last was refused), tried once more with a
/// new token when YouTube says 401.
pub struct Api<'a> {
    pub agent: &'a ureq::Agent,
    pub ends: &'a Endpoints,
    pub token: &'a (dyn Fn(bool) -> anyhow::Result<String> + Sync),
}

impl Api<'_> {
    pub fn url(&self, path: &str) -> String {
        format!("{}/youtube/v3/{path}", self.ends.api)
    }

    pub fn upload_url(&self, path: &str) -> String {
        format!("{}/upload/youtube/v3/{path}", self.ends.api)
    }

    /// One request, made by `make` with the token's header, again with a new token after a 401.
    /// `make` sends the request with the header's value, its answer through [`send`].
    pub fn call(&self, make: impl Fn(&str) -> Result<ureq::Response, Failure>) -> anyhow::Result<ureq::Response> {
        let auth = format!("Bearer {}", (self.token)(false)?);
        match make(&auth) {
            Err(Failure::Said { status: 401, .. }) => {
                let auth = format!("Bearer {}", (self.token)(true)?);
                Ok(make(&auth)?)
            }
            got => Ok(got?),
        }
    }

    pub fn get(&self, path: &str, query: &[(&str, &str)]) -> anyhow::Result<Value> {
        let url = self.url(path);
        let r = self.call(|auth| {
            let mut req = self.agent.get(&url).set("Authorization", auth);
            for (k, v) in query {
                req = req.query(k, v);
            }
            send(req.call())
        })?;
        Ok(json(r)?)
    }

    pub fn send_json(&self, method: &str, path: &str, query: &[(&str, &str)], body: &Value) -> anyhow::Result<Value> {
        let url = self.url(path);
        let r = self.call(|auth| {
            let mut req = self.agent.request(method, &url).set("Authorization", auth);
            for (k, v) in query {
                req = req.query(k, v);
            }
            send(req.send_json(body.clone()))
        })?;
        Ok(json(r)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_is_its_id_and_secret() {
        let c = OAuthClient::parse("123-abc9.apps.googleusercontent.com GOCSPX-abcdefghijklmnop_-12").unwrap();
        assert_eq!(c.id, "123-abc9.apps.googleusercontent.com");
        assert_eq!(c.secret, "GOCSPX-abcdefghijklmnop_-12");
        assert!(OAuthClient::parse("123-abc9.apps.googleusercontent.com:GOCSPX-abcdefghijklmnop").is_some());
        assert!(OAuthClient::parse("GOCSPX-abcdefghijklmnop").is_none());
        assert!(!format!("{c:?}").contains("GOCSPX"), "its Debug leaves the secret out");
    }

    #[test]
    fn googles_errors_say_their_reason() {
        let api = r#"{"error": {"code": 403, "message": "The request cannot be completed because you have exceeded your quota.", "errors": [{"reason": "quotaExceeded"}]}}"#;
        let e = said(403, api).to_string();
        assert!(e.contains("quotaExceeded") && e.contains("midnight Pacific"), "{e}");
        let oauth = r#"{"error": "invalid_grant", "error_description": "Token has been expired or revoked."}"#;
        assert!(matches!(said(400, oauth), Failure::Said { reason, .. } if reason == "invalid_grant"));
    }
}
