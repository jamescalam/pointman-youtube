//! Live chat under a stand-in node, against a fake YouTube (tests/common): reading a chat page by page, watching it
//! into a thread (and riding out a drop), and the moderator's commands, which do nothing without `confirm`. The
//! stand-in answers `board.post` itself and keeps what it was asked to post.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Fake, CLIENT_ID, CLIENT_SECRET, REFRESH};
use pointman_extension::protocol::{BoardPostRequest, BoardPostResponse};
use pointman_extension::testing::{RunOptions, StandIn};
use pointman_extension::v1::PostRef;
use serde_json::{json, Value};

const VIDEO: &str = "live0000001";
const CHAT: &str = "Cg0KC2xpdmUwMDAwMDAx";
const SPAMMER: &str = "UCspamspamspamspamspam01";

/// What the extension posted: (thread, text, its blocks as JSON).
type Posts = Arc<Mutex<Vec<(String, String, Value)>>>;

fn start(fake: &Fake) -> (StandIn, Posts) {
    let posts: Posts = Arc::default();
    let p = posts.clone();
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_youtube"))
        .secrets([("youtube.client", format!("{CLIENT_ID} {CLIENT_SECRET}")), ("youtube.main", REFRESH.to_string())])
        .env("YOUTUBE_API_BASE", &fake.url)
        .env("YOUTUBE_OAUTH_BASE", &fake.url)
        .on(move |r: BoardPostRequest| {
            let mut p = p.lock().unwrap();
            let thread = r.thread.as_option().map(|t| t.id.clone()).unwrap_or_default();
            p.push((thread, r.text.clone(), serde_json::to_value(&r.blocks).unwrap()));
            let post = PostRef { id: format!("post_{}", 900 + p.len()), ..Default::default() };
            Ok(BoardPostResponse { post: post.into(), ..Default::default() })
        })
        .start()
        .unwrap();
    (node, posts)
}

fn live_fake() -> Fake {
    let fake = Fake::start();
    fake.add_live(VIDEO, "Read less, know more", CHAT);
    fake.say(CHAT, "LCC.1", "Ana", "UCanaanaanaanaanaanaana1", "hello from Lisbon", "2026-11-18T14:00:05Z");
    fake.say(CHAT, "LCC.2", "Ben", "UCbenbenbenbenbenbenben1", "@james can you zoom in?", "2026-11-18T14:00:09Z");
    fake
}

fn wait_for(posts: &Posts, n: usize) -> Vec<(String, String, Value)> {
    for _ in 0..200 {
        if posts.lock().unwrap().len() >= n {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    posts.lock().unwrap().clone()
}

#[test]
fn live_lists_broadcasts_and_says_whether_it_may_moderate() {
    let fake = live_fake();
    fake.state().broadcasts = vec![
        json!({"_status": "active", "id": VIDEO, "snippet": {"title": "Read less, know more", "liveChatId": CHAT,
               "actualStartTime": "2026-11-18T14:00:00Z", "scheduledStartTime": "2026-11-18T14:00:00Z"}}),
        json!({"_status": "upcoming", "id": "next0000001", "snippet": {"title": "Rehearsal", "liveChatId": "Cgnext",
               "scheduledStartTime": "2026-11-19T03:00:00Z"}}),
    ];
    let (node, _) = start(&fake);
    let done = node.run("youtube.live", json!({})).unwrap().result;
    let b = done["broadcasts"].as_array().unwrap();
    assert_eq!(b.len(), 2);
    assert_eq!(
        (b[0]["video"].as_str(), b[0]["status"].as_str(), b[0]["chat_id"].as_str()),
        (Some(VIDEO), Some("live"), Some(CHAT))
    );
    assert_eq!(b[1]["status"], "upcoming");
    assert_eq!(done["can_moderate"], true, "{done}");
    assert_eq!(done["scopes"], json!(["https://www.googleapis.com/auth/youtube"]));

    let fake = live_fake();
    fake.state().scope = Some("https://www.googleapis.com/auth/youtube.readonly".into());
    let (node, _) = start(&fake);
    let done = node.run("youtube.live", json!({})).unwrap().result;
    assert_ne!(done["can_moderate"], true);
    assert!(done["text"].as_str().unwrap().contains("can't moderate"), "{done}");
}

#[test]
fn chat_reads_a_page_then_only_whats_new() {
    let fake = live_fake();
    let (node, _) = start(&fake);
    let first = node.run("youtube.chat", json!({"video": VIDEO})).unwrap().result;
    let msgs = first["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["author"], "Ana");
    assert_eq!(msgs[0]["author_channel_id"], "UCanaanaanaanaanaanaana1");
    assert_eq!(msgs[1]["text"], "@james can you zoom in?", "the result keeps the words as they were");
    assert_eq!(first["chat_id"], CHAT);
    assert_eq!(first["poll_ms"], 1000);
    assert!(!first["text"].as_str().unwrap().contains("@james"), "the text can't mention anyone: {first}");

    fake.say(CHAT, "LCC.3", "Cleo", "UCcleocleocleocleocleocl1", "great talk", "2026-11-18T14:00:20Z");
    let next = node.run("youtube.chat", json!({"chat_id": CHAT, "page_token": first["page_token"]})).unwrap().result;
    let msgs = next["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], "LCC.3");
}

#[test]
fn chat_says_when_a_video_isnt_live() {
    let fake = live_fake();
    fake.add_video("notlive0001", "A tutorial", "public", 5);
    let (node, _) = start(&fake);
    let e = node.run("youtube.chat", json!({"video": "notlive0001"})).unwrap_err();
    assert!(e.message.contains("isn't a live stream"), "{e}");
    let e = node.run("youtube.chat", json!({})).unwrap_err();
    assert!(e.message.contains("video") && e.message.contains("chat_id"), "{e}");
}

#[test]
fn chat_watch_posts_new_messages_in_batches_and_rides_out_a_drop() {
    let fake = live_fake();
    let (node, posts) = start(&fake);
    let n = Arc::new(node);
    let runner = n.clone();
    let watch = std::thread::spawn(move || {
        runner.run_with(
            "youtube.chat_watch",
            json!({"video": VIDEO, "thread": "71027", "every_seconds": 5, "mention": "loupe"}),
            RunOptions { timeout: Duration::from_secs(90), cid: Some("watch".into()), ..Default::default() },
        )
    });
    // what was in the chat before it started isn't posted; what comes after is, in one batch
    std::thread::sleep(Duration::from_millis(1500));
    fake.say(CHAT, "LCC.3", "Cleo", "UCcleocleocleocleocleocl1", "/despawn loupe", "2026-11-18T14:00:20Z");
    fake.say(CHAT, "LCC.4", "Dev", "UCdevdevdevdevdevdevdev1", "hi @loupe", "2026-11-18T14:00:21Z");
    let got = wait_for(&posts, 1);
    assert_eq!(got.len(), 1, "{got:?}");
    let (thread, text, blocks) = &got[0];
    assert_eq!(thread, "thr_71027");
    assert!(text.contains("2 new in YouTube chat") && text.ends_with(" @loupe"), "{text}");
    let table = blocks.to_string();
    assert!(table.contains("Cleo") && table.contains("Dev"), "{table}");
    assert!(!table.contains("hello from Lisbon"), "the backlog isn't posted: {table}");
    assert!(!table.contains("@loupe") && !table.contains("\"/despawn"), "chat words can't mention or command: {table}");

    // the stream drops: one line says so; when it's back, another, and the watch goes on
    fake.state().ended.push(CHAT.into());
    let got = wait_for(&posts, 2);
    assert!(got[1].1.contains("dropped"), "{got:?}");
    fake.state().ended.clear();
    let got = wait_for(&posts, 3);
    assert!(got[2].1.contains("is back"), "{got:?}");
    fake.say(CHAT, "LCC.5", "Eve", "UCeveeveeveeveeveeveeve1", "back again", "2026-11-18T14:01:00Z");
    let got = wait_for(&posts, 4);
    assert!(got[3].2.to_string().contains("back again"), "{got:?}");

    assert!(n.cancel("watch").unwrap());
    let done = watch.join().unwrap().unwrap().result;
    assert_eq!(done["stopped"], "cancelled");
    assert_eq!(done["batches"], 2);
    assert_eq!(done["messages"], 3);
}

#[test]
fn moderating_without_confirm_does_nothing() {
    let fake = live_fake();
    let (node, posts) = start(&fake);
    let e = node.run("youtube.chat_send", json!({"video": VIDEO, "text": "Welcome!"})).unwrap_err();
    assert!(e.message.contains("confirm=true"), "{e}");
    let e = node
        .run("youtube.chat_remove", json!({"video": VIDEO, "message_id": "LCC.2", "reason": "spam", "thread": "71027"}))
        .unwrap_err();
    assert!(e.message.contains("confirm=true"), "{e}");
    let e = node
        .run("youtube.chat_timeout", json!({"video": VIDEO, "author_channel_id": SPAMMER, "seconds": 300}))
        .unwrap_err();
    assert!(e.message.contains("confirm=true"), "{e}");
    assert!(fake.requests().is_empty(), "nothing reached YouTube: {:?}", fake.requests());
    assert!(posts.lock().unwrap().is_empty());
}

#[test]
fn chat_remove_deletes_the_message_and_logs_who_what_when_and_why() {
    let fake = live_fake();
    fake.say(CHAT, "LCC.spam", "Crypto King", SPAMMER, "FREE BTC at scam.example @james", "2026-11-18T14:02:03Z");
    let (node, posts) = start(&fake);
    let done = node
        .run(
            "youtube.chat_remove",
            json!({"video": VIDEO, "message_id": "LCC.spam", "reason": "spam link", "thread": "thr_71027", "confirm": true}),
        )
        .unwrap()
        .result;
    assert_eq!(fake.state().deleted, ["LCC.spam"]);
    assert_eq!(done["message"]["author"], "Crypto King");
    assert_eq!(done["logged"], "post_901");
    let got = posts.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let (thread, line, _) = &got[0];
    assert_eq!(thread, "thr_71027");
    for part in ["Crypto King", SPAMMER, "14:02:03 UTC", "FREE BTC at scam.example", "spam link"] {
        assert!(line.contains(part), "{part} in {line}");
    }
    assert!(!line.contains("@james"), "{line}");

    // a message that isn't in the chat is refused before anything is deleted
    let e = node
        .run(
            "youtube.chat_remove",
            json!({"video": VIDEO, "message_id": "LCC.nope", "reason": "spam", "thread": "71027", "confirm": true}),
        )
        .unwrap_err();
    assert!(e.message.contains("isn't among"), "{e}");
    assert_eq!(fake.state().deleted.len(), 1);
}

#[test]
fn chat_send_and_timeout_with_confirm() {
    let fake = live_fake();
    let (node, _) = start(&fake);
    let sent = node.run("youtube.chat_send", json!({"video": VIDEO, "text": "Welcome!", "confirm": true})).unwrap();
    assert!(sent.result["message_id"].as_str().unwrap().starts_with("LCC.sent"));
    let posted = fake.api_requests().into_iter().find(|r| r.method == "POST").unwrap();
    assert_eq!(posted.json["snippet"]["liveChatId"], CHAT);
    assert_eq!(posted.json["snippet"]["textMessageDetails"]["messageText"], "Welcome!");

    let t = node
        .run(
            "youtube.chat_timeout",
            json!({"video": VIDEO, "author_channel_id": SPAMMER, "seconds": 300, "confirm": true}),
        )
        .unwrap();
    assert_eq!(t.result["ban_id"], "ban1");
    let ban = &fake.state().bans[0];
    assert_eq!(ban["snippet"]["type"], "temporary");
    assert_eq!(ban["snippet"]["banDurationSeconds"], 300);
    assert_eq!(ban["snippet"]["bannedUserDetails"]["channelId"], SPAMMER);
    assert_eq!(ban["snippet"]["liveChatId"], CHAT);
}

#[test]
fn chat_watch_started_before_the_stream_waits_for_its_chat() {
    let fake = Fake::start();
    fake.add_live(VIDEO, "Read less, know more", CHAT);
    // scheduled, not started: YouTube gives no activeLiveChatId yet
    fake.state().videos.get_mut(VIDEO).unwrap()["liveStreamingDetails"] =
        json!({"scheduledStartTime": "2026-11-19T02:40:00Z"});
    let (node, posts) = start(&fake);
    let n = Arc::new(node);
    let runner = n.clone();
    let watch = std::thread::spawn(move || {
        runner.run_with(
            "youtube.chat_watch",
            json!({"video": VIDEO, "thread": "71027", "every_seconds": 5}),
            RunOptions { timeout: Duration::from_secs(60), cid: Some("early".into()), ..Default::default() },
        )
    });
    let got = wait_for(&posts, 1);
    assert!(got[0].1.contains("Waiting for the stream's chat to open"), "{got:?}");
    // the stream starts
    fake.state().videos.get_mut(VIDEO).unwrap()["liveStreamingDetails"] =
        json!({"actualStartTime": "2026-11-19T02:41:00Z", "activeLiveChatId": CHAT});
    let got = wait_for(&posts, 2);
    assert!(got[1].1.contains("chat is open"), "{got:?}");
    fake.say(CHAT, "LCC.9", "Ana", "UCanaanaanaanaanaanaana1", "first!", "2026-11-19T02:41:05Z");
    let got = wait_for(&posts, 3);
    assert!(got[2].2.to_string().contains("first!"), "{got:?}");
    assert!(n.cancel("early").unwrap());
    let done = watch.join().unwrap().unwrap().result;
    assert_eq!(done["stopped"], "cancelled");
    assert_eq!(done["messages"], 1);
}

#[test]
fn chat_watch_on_a_video_that_isnt_live_fails_at_once() {
    let fake = Fake::start();
    fake.add_video("notlive0001", "A tutorial", "public", 5);
    let (node, posts) = start(&fake);
    let e = node.run("youtube.chat_watch", json!({"video": "notlive0001", "thread": "71027"})).unwrap_err();
    assert!(e.message.contains("isn't a live stream"), "{e}");
    assert!(posts.lock().unwrap().is_empty());
}

#[test]
fn live_on_a_channel_that_cant_stream_says_so_and_still_gives_the_scopes() {
    let fake = live_fake();
    fake.state().no_live = true;
    let (node, _) = start(&fake);
    let done = node.run("youtube.live", json!({})).unwrap().result;
    assert_ne!(done["can_stream"], true, "{done}");
    assert!(done["text"].as_str().unwrap().contains("isn't enabled for live streaming"), "{done}");
    assert_eq!(done["can_moderate"], true, "{done}");
    // and a channel that can: can_stream
    let fake = live_fake();
    let (node, _) = start(&fake);
    assert_eq!(node.run("youtube.live", json!({})).unwrap().result["can_stream"], true);
}
