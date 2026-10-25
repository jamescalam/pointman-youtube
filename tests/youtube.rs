//! The extension's commands under a stand-in node, against a fake YouTube (tests/common): uploads that resume, changes
//! that keep what they weren't asked to change, and nothing reaching the channel without `confirm`. No real channel is
//! called: the extension is pointed at the fake through YOUTUBE_API_BASE and YOUTUBE_OAUTH_BASE.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Fake, CLIENT_ID, CLIENT_SECRET, QUANTUM, REFRESH, USER_CODE};
use pointman_extension::testing::{RunOptions, StandIn};
use serde_json::{json, Value};

const MIB: usize = 1024 * 1024;

fn node_with(fake: &Fake, dir: &Path, settings: Value, secrets: Vec<(&str, String)>) -> StandIn {
    StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_youtube"))
        .secrets(secrets)
        .settings(settings)
        .env("YOUTUBE_API_BASE", &fake.url)
        .env("YOUTUBE_OAUTH_BASE", &fake.url)
        .roots([dir.to_path_buf()])
        .start()
        .unwrap()
}

fn signed_in() -> Vec<(&'static str, String)> {
    vec![("youtube.client", format!("{CLIENT_ID} {CLIENT_SECRET}")), ("youtube.main", REFRESH.to_string())]
}

fn node(fake: &Fake, dir: &Path) -> StandIn {
    node_with(fake, dir, json!({"chunk_mib": 1}), signed_in())
}

fn video_file(dir: &Path, name: &str, size: usize) -> (PathBuf, Vec<u8>) {
    let data = common::bytes(size);
    let path = dir.join(name);
    std::fs::write(&path, &data).unwrap();
    (path, data)
}

fn png(dir: &Path) -> PathBuf {
    let path = dir.join("thumb.png");
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend(common::bytes(4000));
    std::fs::write(&path, data).unwrap();
    path
}

#[test]
fn an_upload_without_confirm_is_refused_before_anything_reaches_youtube() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = video_file(dir.path(), "final.mp4", 300_000);
    let node = node(&fake, dir.path());
    for confirm in [json!(null), json!(false)] {
        let mut params = json!({"file": path, "title": "Claude Code in 10 minutes", "privacy": "public"});
        if !confirm.is_null() {
            params["confirm"] = confirm;
        }
        let e = node.run("youtube.upload", params).unwrap_err();
        assert!(e.message.contains("confirm=true"), "{e}");
        assert!(e.message.contains("public"), "it says what would have gone up: {e}");
    }
    assert!(fake.requests().is_empty(), "no token asked for, no upload started: {:?}", fake.requests());
}

#[test]
fn an_upload_resumes_after_a_dropped_connection() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let size = 2 * MIB + MIB / 2;
    let (path, data) = video_file(dir.path(), "final.mp4", size);
    // the connection drops 300 KiB into the second 1 MiB chunk; YouTube keeps the first whole 256 KiB of it
    fake.state().drop_at = Some(MIB + 300 * 1024);
    let thumb = png(dir.path());
    let node = node(&fake, dir.path());
    let done = node
        .run_timeout(
            "youtube.upload",
            json!({
                "file": path, "title": "Claude Code in 10 minutes", "description": "What it does.\n\n0:00 Intro",
                "tags": ["claude code", "ai"], "category": "science_technology", "made_for_kids": false,
                "thumbnail": thumb, "playlist": "PLfake123", "confirm": true,
            }),
            Duration::from_secs(60),
        )
        .unwrap();
    let id = done.result["video"].as_str().unwrap().to_string();
    assert_eq!(done.result["url"], format!("https://youtu.be/{id}"));
    assert_eq!(done.result["privacy"], "private", "private unless said");
    assert_eq!(done.result["bytes"], size as u64);
    assert_eq!(done.result["thumbnail"], true);
    assert_eq!(done.result["playlist"], "PLfake123");

    let s = fake.state();
    let up = s.uploads.values().next().unwrap();
    assert_eq!(s.uploads.len(), 1, "one session, resumed rather than started again");
    assert!(up.data == data, "YouTube has the file byte for byte");
    let v = &s.videos[&id];
    assert_eq!(v["snippet"]["title"], "Claude Code in 10 minutes");
    assert_eq!(v["snippet"]["categoryId"], "28");
    assert_eq!(v["snippet"]["tags"], json!(["claude code", "ai"]));
    assert_eq!(v["status"]["privacyStatus"], "private");
    assert_eq!(v["status"]["selfDeclaredMadeForKids"], false);
    assert_eq!(s.thumbnails[&id].0, "image/png");
    assert_eq!(s.playlist_items, vec![("PLfake123".to_string(), id.clone())]);
    drop(s);

    let puts: Vec<_> = fake.requests().into_iter().filter(|r| r.path.starts_with("/upload/session/")).collect();
    let ranges: Vec<String> = puts.iter().map(|r| r.headers["content-range"].clone()).collect();
    let size = size as u64;
    let kept = MIB + QUANTUM;
    assert_eq!(
        ranges,
        vec![
            format!("bytes 0-{}/{size}", MIB - 1),
            format!("bytes {MIB}-{}/{size}", 2 * MIB - 1),
            format!("bytes */{size}"),
            format!("bytes {kept}-{}/{size}", 2 * MIB - 1),
            format!("bytes {}-{}/{size}", 2 * MIB, size - 1),
        ],
        "after the drop it asks how much YouTube has, then sends from there to the chunk's boundary"
    );
    assert_eq!(puts[1].read, 300 * 1024, "the second chunk's connection dropped partway");
    let stages: Vec<String> = done.progress.iter().filter_map(|p| p.stage.clone()).collect();
    assert!(stages.iter().any(|s| s.contains("uploading final.mp4")), "{stages:?}");
    assert!(node.log().contains("asking how much YouTube has"), "{}", node.log());
}

#[test]
fn an_upload_carries_on_from_its_checkpoint_after_a_restart() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let (path, data) = video_file(dir.path(), "final.mp4", 2 * MIB);
    let node = node(&fake, dir.path());
    // the first run's session, with the first MiB in it, and the checkpoint it saved
    let done = {
        let mut s = fake.state();
        s.made += 1;
        s.uploads.insert(
            "s1".into(),
            common::Upload {
                size: data.len(),
                data: data[..MIB].to_vec(),
                details: json!({"snippet": {"title": "Resumed", "categoryId": "22"}, "status": {"privacyStatus": "private"}}),
                video: None,
            },
        );
        drop(s);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let modified = modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // the path as the extension resolved and saved it (/private/var on a Mac, for /var)
        let saved = path.canonicalize().unwrap().display().to_string();
        let checkpoint = json!({"url": format!("{}/upload/session/s1", fake.url), "file": saved,
                                "size": data.len(), "modified": modified});
        node.run_with(
            "youtube.upload",
            json!({"file": path, "title": "Resumed", "confirm": true}),
            RunOptions { checkpoint: Some(checkpoint), timeout: Duration::from_secs(60), ..Default::default() },
        )
        .unwrap()
    };
    assert!(fake.state().uploads["s1"].data == data);
    let reqs = fake.api_requests();
    assert!(!reqs.iter().any(|r| r.path == "/upload/youtube/v3/videos"), "no new session");
    let ranges: Vec<String> = reqs.iter().map(|r| r.headers["content-range"].clone()).collect();
    assert_eq!(ranges, vec![format!("bytes */{}", 2 * MIB), format!("bytes {MIB}-{}/{}", 2 * MIB - 1, 2 * MIB)]);
    let id = done.result["video"].as_str().unwrap();
    assert_eq!(fake.video(id)["snippet"]["title"], "Resumed", "the session's details made the video");
}

#[test]
fn a_change_keeps_every_field_it_wasnt_asked_to_change() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    fake.add_video("abcdefghijk", "Old title", "private", 10);
    let before = fake.video("abcdefghijk");
    let node = node(&fake, dir.path());
    let done = node.run("youtube.update", json!({"video": "abcdefghijk", "title": "New title"})).unwrap();
    assert_eq!(done.result["changed"], json!(["title"]));
    assert!(done.result["text"].as_str().unwrap().contains("Changed the title of \"New title\""), "{}", done.result);

    let after = fake.video("abcdefghijk");
    assert_eq!(after["snippet"]["title"], "New title");
    let mut expected = before.clone();
    expected["snippet"]["title"] = json!("New title");
    assert_eq!(after, expected, "the description, tags, category, languages and status are as they were");
    let put = fake.api_requests().into_iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(put.query["part"], "snippet", "status isn't sent when nothing in it changes");

    // the description and tags, then the category, privacy and made-for-kids answer, one change at a time
    let new_description = "A new description.\n\nChapters:\n0:00 Intro";
    node.run("youtube.update", json!({"video": "abcdefghijk", "description": new_description, "tags": ["rust"]}))
        .unwrap();
    let done = node
        .run(
            "youtube.update",
            json!({"video": "abcdefghijk", "category": "education", "privacy": "unlisted", "made_for_kids": false}),
        )
        .unwrap();
    assert_eq!(done.result["changed"], json!(["category", "privacy"]), "made_for_kids was false already");
    let after = fake.video("abcdefghijk");
    assert_eq!(after["snippet"]["title"], "New title");
    assert_eq!(after["snippet"]["description"], new_description);
    assert_eq!(after["snippet"]["tags"], json!(["rust"]));
    assert_eq!(after["snippet"]["categoryId"], "27");
    assert_eq!(after["snippet"]["defaultLanguage"], "en");
    assert_eq!(after["status"]["privacyStatus"], "unlisted");
    assert_eq!(after["status"]["embeddable"], true);
    assert_eq!(after["status"]["license"], "youtube");
    assert_eq!(after["status"]["publicStatsViewable"], true);
}

#[test]
fn a_public_video_is_changed_only_with_confirm() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    fake.add_video("publicvid01", "Live one", "public", 5000);
    fake.add_video("privatevid1", "Draft", "private", 0);
    let thumb = png(dir.path());
    let node = node(&fake, dir.path());
    let refused = [
        ("youtube.update", json!({"video": "publicvid01", "description": "changed"}), "it's public"),
        ("youtube.update", json!({"video": "privatevid1", "privacy": "public"}), "this makes it public"),
        (
            "youtube.update",
            json!({"video": "privatevid1", "publish_at": "2999-01-01T15:00:00Z"}),
            "this schedules it to go public",
        ),
        ("youtube.thumbnail", json!({"video": "publicvid01", "file": thumb}), "is public"),
    ];
    for (kind, params, why) in refused {
        let e = node.run(kind, params.clone()).unwrap_err();
        assert!(e.message.contains("confirm=true") && e.message.contains(why), "{kind} {params}: {e}");
    }
    let writes =
        |f: &Fake| f.api_requests().into_iter().filter(|r| r.method != "GET").map(|r| r.path).collect::<Vec<_>>();
    assert!(writes(&fake).is_empty(), "nothing was changed: {:?}", writes(&fake));
    assert_eq!(fake.video("publicvid01")["snippet"]["description"], "About Live one.\n\nLinks:\n- https://example.com");

    // a private video's details and thumbnail need no confirm; a public one's do, with it
    node.run("youtube.update", json!({"video": "privatevid1", "title": "Draft 2"})).unwrap();
    node.run("youtube.thumbnail", json!({"video": "privatevid1", "file": thumb})).unwrap();
    node.run("youtube.update", json!({"video": "publicvid01", "description": "changed", "confirm": true})).unwrap();
    node.run("youtube.thumbnail", json!({"video": "publicvid01", "file": thumb, "confirm": true})).unwrap();
    let done = node
        .run("youtube.update", json!({"video": "privatevid1", "publish_at": "2999-01-01T15:00:00Z", "confirm": true}))
        .unwrap();
    assert_eq!(done.result["changed"], json!(["publish_at"]));
    assert_eq!(fake.video("privatevid1")["status"]["publishAt"], "2999-01-01T15:00:00Z");
    assert_eq!(fake.video("publicvid01")["snippet"]["description"], "changed");
    assert_eq!(fake.state().thumbnails.len(), 2);
}

#[test]
fn a_change_that_breaks_youtubes_rules_is_refused_before_it_is_sent() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    fake.add_video("abcdefghijk", "Old", "unlisted", 0);
    let node = node(&fake, dir.path());
    let refused = [
        (json!({"video": "abcdefghijk", "title": "a <b> c"}), "< and >"),
        (json!({"video": "abcdefghijk", "title": "x".repeat(101)}), "1 to 100 characters"),
        (json!({"video": "abcdefghijk", "tags": vec!["long tag words".to_string(); 40]}), "500"),
        (json!({"video": "abcdefghijk", "publish_at": "2999-01-01T15:00:00Z", "confirm": true}), "privacy=private"),
        (json!({"video": "abcdefghijk", "publish_at": "2020-01-01T15:00:00Z"}), "has passed"),
        (json!({"video": "https://youtu.be/abcdefghijk"}), "isn't a video id"),
        (json!({"video": "abcdefghijk", "titel": "typo"}), "unknown field"),
    ];
    for (params, why) in refused {
        let e = node.run("youtube.update", params.clone()).unwrap_err();
        assert!(e.message.contains(why), "{params}: {e}");
    }
    assert!(!fake.api_requests().iter().any(|r| r.method == "PUT"));
    let done = node.run("youtube.update", json!({"video": "abcdefghijk", "title": "Old"})).unwrap();
    assert!(done.result["text"].as_str().unwrap().starts_with("Nothing to change"), "{}", done.result);
}

#[test]
fn videos_lists_the_channel_newest_first_a_page_at_a_time() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    for i in 1..=5 {
        fake.add_video(
            &format!("video{i:06}"),
            &format!("Video {i}"),
            if i == 5 { "private" } else { "public" },
            i * 1000,
        );
    }
    let node = node(&fake, dir.path());
    let first = node.run("youtube.videos", json!({"page_size": 3})).unwrap().result;
    let ids: Vec<&str> = first["videos"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["video000005", "video000004", "video000003"]);
    assert_eq!(first["total"], 5);
    assert_eq!(first["videos"][0]["privacy"], "private");
    assert_eq!(first["videos"][1]["views"], 4000);
    assert_eq!(first["videos"][1]["url"], "https://youtu.be/video000004");
    let text = first["text"].as_str().unwrap();
    assert!(text.starts_with("3 of the channel's 5 videos, newest first:"), "{text}");
    assert!(text.contains("- Video 4 (public, 4,000 views, 1 Oct 2026): video000004"), "{text}");
    let next = first["next_page"].as_str().unwrap();
    let second = node.run("youtube.videos", json!({"page_size": 3, "page": next})).unwrap().result;
    let ids: Vec<&str> = second["videos"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["video000002", "video000001"]);
    assert_eq!(second["next_page"], "");

    let one = node.run("youtube.video", json!({"video": "video000004"})).unwrap().result;
    assert_eq!(one["title"], "Video 4");
    assert_eq!(one["category"], "science_technology");
    assert_eq!(one["tags"], json!(["ai", "claude code"]));
    assert_eq!(one["made_for_kids"], false);
    assert_eq!(one["duration"], "PT12M3S");
}

#[test]
fn a_channel_signs_in_with_googles_device_flow() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let client_only = vec![("youtube.client", format!("{CLIENT_ID} {CLIENT_SECRET}"))];
    let node = node_with(&fake, dir.path(), json!({}), client_only);
    let started = node
        .call(
            "credentials.sign_in",
            json!({"form": "account", "label": "main", "person": {"handle": "james"}, "session": {"value": "sgn_1"}}),
        )
        .unwrap();
    assert_eq!(started["url"], "https://www.google.com/device");
    assert_eq!(started["code"], USER_CODE);
    assert_eq!(started["waits"], true);
    assert!(started["prompt"].as_str().unwrap().contains(USER_CODE));
    let got = node
        .call_timeout(
            "credentials.wait",
            json!({"form": "account", "session": {"value": "sgn_1"}}),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(got["value"], REFRESH, "the vault's value is the refresh token alone");
    let polls = fake.requests().iter().filter(|r| r.path == "/token").count();
    assert_eq!(polls, 2, "it waited while Google said authorization_pending");
    // the session is over: waiting again says so
    let e = node.call("credentials.wait", json!({"form": "account", "session": {"value": "sgn_1"}})).unwrap_err();
    assert!(e.message.contains("ended"), "{e}");

    // the client's own name isn't a channel's, and with no client there's nothing to sign in with
    let e = node
        .call("credentials.sign_in", json!({"form": "account", "label": "client", "session": {"value": "sgn_2"}}))
        .unwrap_err();
    assert!(e.message.contains("youtube.client"), "{e}");
    let bare = node_with(&fake, dir.path(), json!({}), vec![]);
    let e = bare
        .call("credentials.sign_in", json!({"form": "account", "label": "main", "session": {"value": "sgn_3"}}))
        .unwrap_err();
    assert!(e.message.contains("/youtube client"), "{e}");
}

#[test]
fn a_run_out_token_is_made_again_and_no_secret_reaches_the_log_or_an_answer() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    fake.add_video("abcdefghijk", "Old", "private", 0);
    let node = node(&fake, dir.path());
    node.run("youtube.video", json!({"video": "abcdefghijk"})).unwrap();
    // YouTube says the token it gave has run out: the next call makes a new one and tries again
    let first = fake.state().tokens[0].clone();
    fake.state().expire.push(first.clone());
    let done = node.run("youtube.update", json!({"video": "abcdefghijk", "title": "New"})).unwrap();
    assert_eq!(fake.state().tokens.len(), 2);
    assert_eq!(fake.video("abcdefghijk")["snippet"]["title"], "New");

    // no channel signed in: it says how to sign one in
    let none =
        node_with(&fake, dir.path(), json!({}), vec![("youtube.client", format!("{CLIENT_ID} {CLIENT_SECRET}"))]);
    let e = none.run("youtube.videos", json!({})).unwrap_err();
    assert!(e.message.contains("/youtube add"), "{e}");
    let e = node_with(&fake, dir.path(), json!({"account": "work"}), signed_in())
        .run("youtube.videos", json!({}))
        .unwrap_err();
    assert!(e.message.contains("youtube.work") && e.message.contains("youtube.main"), "{e}");

    let said = format!("{}\n{}", node.log(), done.result);
    for secret in [REFRESH, CLIENT_SECRET, first.as_str()] {
        assert!(!said.contains(secret), "{secret} is in the log or an answer");
    }
}

#[test]
fn files_come_from_the_folders_the_node_allows() {
    let fake = Fake::start();
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let (outside, _) = video_file(elsewhere.path(), "secret.mp4", 1000);
    fake.add_video("abcdefghijk", "Old", "private", 0);
    let node = node(&fake, dir.path());
    let e = node.run("youtube.upload", json!({"file": outside, "title": "x", "confirm": true})).unwrap_err();
    assert!(e.message.contains("outside the folders"), "{e}");
    let e =
        node.run("youtube.upload", json!({"file": "pointman://asset/12", "title": "x", "confirm": true})).unwrap_err();
    assert!(e.message.contains("wasn't fetched by this node"), "{e}");
    let gif = dir.path().join("t.gif");
    std::fs::write(&gif, b"GIF89a....").unwrap();
    let e = node.run("youtube.thumbnail", json!({"video": "abcdefghijk", "file": gif})).unwrap_err();
    assert!(e.message.contains("isn't a JPEG or PNG"), "{e}");
    let big = dir.path().join("big.png");
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.resize(2 * MIB + 1, 0);
    std::fs::write(&big, data).unwrap();
    let e = node.run("youtube.thumbnail", json!({"video": "abcdefghijk", "file": big})).unwrap_err();
    assert!(e.message.contains("at most 2 MB"), "{e}");
    assert!(fake.api_requests().is_empty());
}
