//! Signing a YouTube channel in from the apps with Google's device flow
//! (https://developers.google.com/identity/protocols/oauth2/limited-input-device): the person opens google.com/device
//! anywhere, enters the code the form shows, and picks the channel's Google account. The sign-in waits
//! (`credentials.wait`) until Google says yes; a node that asks for the code back instead (`credentials.code`) gets the
//! same, once the code it was given matches.
//!
//! The value that goes in the vault is the refresh token, alone, as the form's `value` matches. Nothing here writes it
//! anywhere else, logs it or keeps it once it's given.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use pointman_extension::protocol::CredentialsSignInResponse;

use crate::google::{json, send, Endpoints, Failure, OAuthClient, SCOPE};

/// How long a sign-in waits for the person, at most: under the node's own wait for `credentials.wait` (14 minutes).
pub const WAIT: Duration = Duration::from_secs(13 * 60);

struct Pending {
    client: OAuthClient,
    device_code: String,
    user_code: String,
    interval: Duration,
    until: Instant,
}

/// The sign-ins under way, by the node's session id.
#[derive(Default)]
pub struct SignIns {
    open: Mutex<HashMap<String, Pending>>,
}

impl SignIns {
    fn take(&self, session: &str) -> anyhow::Result<Pending> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.retain(|_, p| p.until > Instant::now());
        open.remove(session).ok_or_else(|| anyhow!("this sign-in has ended: start it again"))
    }

    /// Start one: Google's link and the code to enter there.
    pub fn start(
        &self,
        agent: &ureq::Agent,
        ends: &Endpoints,
        client: OAuthClient,
        session: &str,
    ) -> anyhow::Result<CredentialsSignInResponse> {
        if session.is_empty() {
            bail!("a sign-in needs the node's session");
        }
        let got = send(
            agent
                .post(&format!("{}/device/code", ends.oauth))
                .send_form(&[("client_id", client.id.as_str()), ("scope", SCOPE)]),
        );
        let v = match got {
            Ok(r) => json(r)?,
            Err(Failure::Said { reason, .. }) if reason == "invalid_client" || reason == "unauthorized_client" => bail!(
                "Google refused the OAuth client ({reason}): youtube.client must be a \"TVs and Limited Input devices\" \
                 client, in a project with the YouTube Data API v3 turned on"
            ),
            Err(e) => return Err(e.into()),
        };
        let field = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        let (Some(device_code), Some(user_code)) = (field("device_code"), field("user_code")) else {
            bail!("Google gave no sign-in code");
        };
        let url = field("verification_url")
            .or_else(|| field("verification_uri"))
            .unwrap_or("https://www.google.com/device".into());
        if !url.starts_with("https://") {
            bail!("Google's sign-in link isn't https: {url}");
        }
        let expires = Duration::from_secs(v["expires_in"].as_u64().unwrap_or(1800));
        let interval = Duration::from_secs(v["interval"].as_u64().unwrap_or(5).max(1));
        let prompt = format!("Enter {user_code} at {url}, signed in as the Google account that owns the channel");
        self.open.lock().unwrap_or_else(|e| e.into_inner()).insert(
            session.to_string(),
            Pending { client, device_code, user_code: user_code.clone(), interval, until: Instant::now() + expires },
        );
        Ok(CredentialsSignInResponse {
            url,
            prompt: Some(prompt),
            waits: true,
            code: Some(user_code),
            ..Default::default()
        })
    }

    /// The node waits for this sign-in: the refresh token once Google says yes.
    pub fn wait(
        &self,
        agent: &ureq::Agent,
        ends: &Endpoints,
        session: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let p = self.take(session)?;
        poll(agent, ends, &p, timeout)
    }

    /// The person pasted the code they entered at Google: the refresh token once Google says yes.
    pub fn code(
        &self,
        agent: &ureq::Agent,
        ends: &Endpoints,
        session: &str,
        code: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let p = self.take(session)?;
        let given: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let want: String = p.user_code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if !given.eq_ignore_ascii_case(&want) {
            let again = p.user_code.clone();
            self.open.lock().unwrap_or_else(|e| e.into_inner()).insert(session.to_string(), p);
            bail!("that isn't this sign-in's code: paste {again}, the one you entered at Google");
        }
        poll(agent, ends, &p, timeout)
    }
}

/// Ask Google every `interval` until the person has said yes (or no), the code has run out, or `timeout` has passed.
fn poll(agent: &ureq::Agent, ends: &Endpoints, p: &Pending, timeout: Duration) -> anyhow::Result<String> {
    let end = (Instant::now() + timeout).min(p.until);
    let mut interval = p.interval;
    loop {
        let got = send(agent.post(&format!("{}/token", ends.oauth)).send_form(&[
            ("client_id", p.client.id.as_str()),
            ("client_secret", p.client.secret.as_str()),
            ("device_code", p.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ]));
        match got {
            Ok(r) => {
                let v = json(r)?;
                let Some(refresh) = v["refresh_token"].as_str().filter(|t| !t.is_empty()) else {
                    bail!("Google signed it in but gave no refresh token");
                };
                let scopes = v["scope"].as_str().unwrap_or_default();
                if !scopes.split(' ').any(|s| s == SCOPE) {
                    bail!(
                        "the sign-in didn't allow managing the channel's videos ({SCOPE}): sign in again and allow it"
                    );
                }
                return Ok(refresh.to_string());
            }
            Err(Failure::Said { reason, .. }) if reason == "authorization_pending" => {}
            Err(Failure::Said { reason, .. }) if reason == "slow_down" => interval += Duration::from_secs(5),
            Err(Failure::Said { reason, .. }) if reason == "access_denied" => {
                bail!("the sign-in was turned down at Google")
            }
            Err(Failure::Said { reason, .. }) if reason == "expired_token" => {
                bail!("the code {} ran out before it was entered: start again", p.user_code)
            }
            Err(Failure::Lost(why)) => eprintln!("no answer from Google while waiting for the sign-in: {why}"),
            Err(e) => return Err(e.into()),
        }
        if Instant::now() + interval > end {
            bail!("Google hasn't said yes yet: enter {} at google.com/device, then start again", p.user_code);
        }
        std::thread::sleep(interval);
    }
}
