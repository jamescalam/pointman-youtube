# youtube

A Pointman extension for agents that publish YouTube videos. Its behaviour covers the research, titles,
description, chapters, tags and thumbnails for an upload, made in a thread and picked by the channel's
owner. Its process uploads the video and changes a video's details on the channel through the YouTube
Data API v3, only once the owner has said yes. It works for any channel: the owner's brand, links and
taste are collected once and kept in the channel's own folder.

| File | What | When the agent reads it |
|---|---|---|
| `SKILL.md` | The workflow: research, drafts, phone check, the owner picks; the channel commands | Every wake (kept short) |
| `references/packaging.md` | Titles, description, chapters, tags, pinned comment, social posts | When it's writing those |
| `references/thumbnails.md` | Research, faces and cutouts, layout, rendering, the feed check, A/B tests | When it's making thumbnails |
| `scripts/thumbs.py` | Channel research, HTML to PNG renders, the feed-size check | Run with `uv run` |
| `evals/` | Tasks that measure it, run with and without the behaviour | When measuring it |
| `proto/youtube/v1/youtube.proto` | The commands' params and results, and the settings | When changing a command |

## Install

```
pointman add jamescalam/pointman-youtube
```

Every agent on the node gets `SKILL.md` in its prompt from its next wake. The commands run on the
machines it's installed on. The behaviour alone is also an ordinary skill: copy `SKILL.md`,
`references/` and `scripts/` to `~/.claude/skills/youtube/`.

## The commands

| Command | What it does |
|---|---|
| `youtube.videos {page_size?, page?}` | The channel's videos, newest first: id, title, privacy, publish time, views |
| `youtube.video {video}` | One video's title, description, tags, category, privacy and schedule |
| `youtube.upload {file, title, description, tags, category, privacy, publish_at?, made_for_kids?, thumbnail?, playlist?, confirm}` | A resumable upload, with progress; private unless said. Refused without `confirm: true` |
| `youtube.update {video, title?, description?, tags?, category?, privacy?, publish_at?, made_for_kids?, confirm?}` | Changes only the fields given. A public video, or making one public or scheduling it, needs `confirm: true` |
| `youtube.thumbnail {video, file, confirm?}` | Sets a JPEG or PNG thumbnail (at most 2 MB). A public video's needs `confirm: true` |
| `/youtube client`, `/youtube add` | Open the forms for the OAuth client and a channel's sign-in in the app |

**Uploads resume.** A video goes up in chunks (`chunk_mib`, 8 MiB by default). After a dropped
connection the extension asks YouTube how much it has and carries on from there. The upload session is
the job's checkpoint, so a node that restarts mid-upload carries on too.

**A change keeps the rest.** YouTube's `videos.update` replaces every field of each part it's sent, so
a change sent with only a title would clear the description and tags. The extension reads the video
first, changes only the fields asked for, and sends back only the parts with a change in them.

## Setting it up

1. In [Google Cloud console](https://console.cloud.google.com/), in a project of yours: turn on the
   **YouTube Data API v3**, set up the OAuth consent screen (External), and add yourself as a user.
   Publish the app to production once it works. While it's in testing, Google's refresh tokens last
   7 days, so the channel would need signing in again every week.
2. **APIs & Services › Credentials › Create OAuth client ID › TVs and Limited Input devices.**
3. In the Pointman app: `/youtube client`, and paste the client ID, a space, then the client secret.
   It's sealed in the vault as `youtube.client`.
4. `/youtube add` and tap **Sign in**. Enter the code at google.com/device, signed in as the Google
   account that owns the channel. The refresh token is sealed in the vault as `youtube.<label>`.

Settings, per machine: `account` (which sign-in to use, by label; the first if empty) and `chunk_mib`.
Uploads have their own daily quota ([videos.insert](https://developers.google.com/youtube/v3/docs/videos/insert));
reads cost from the project's general YouTube quota.

## Building and testing

```
cargo test
```

The tests run the extension under the SDK's stand-in node against a fake YouTube on a local port
(`tests/common`): an upload whose connection drops partway, an upload resumed from its checkpoint,
changes that keep every other field, refusals without `confirm`, the device-flow sign-in, and a token
that has run out. No real channel is called.

`extension.toml`'s params and settings schemas are written from `proto/youtube/v1/youtube.proto`:
`tests/manifest.rs` checks they match it field for field. A release is a tag that's the manifest's
version (`git tag v0.3.0 && git push origin v0.3.0`): `.github/workflows/release.yml` builds the four
platforms and attaches them to the release.

## Requirements

For the behaviour's scripts: `uv`, `yt-dlp`, `ffmpeg` and ImageMagick on the machine. Thumbnails
render with Playwright's Chromium (`uv run --with playwright playwright install chromium`), or with the
Open Design extension where the node has it.

## Sources

YouTube's own help pages for the limits and rules ([A/B tests](https://support.google.com/youtube/answer/16391400),
[chapters](https://support.google.com/youtube/answer/9884579), [hashtags](https://support.google.com/youtube/answer/6390658),
[tags](https://support.google.com/youtube/answer/146402)); the Data API's [video properties](https://developers.google.com/youtube/v3/docs/videos#properties),
[resumable uploads](https://developers.google.com/youtube/v3/guides/using_resumable_upload_protocol) and
[device sign-in](https://developers.google.com/identity/protocols/oauth2/limited-input-device). The rest is
cited where it's used.

MIT licence.
