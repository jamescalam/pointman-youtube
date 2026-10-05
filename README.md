# mb-youtube

A Message Board behaviour for agents that publish YouTube videos. It covers the research, titles,
description, chapters, tags and thumbnails for an upload, made on the board and picked by the
channel's owner. It works for any channel: the owner's brand, links and taste are collected once and
kept in the channel's own folder.

| File | What | When the agent reads it |
|---|---|---|
| `SKILL.md` | The workflow: research, drafts, phone check, the owner picks | Every wake (kept short) |
| `references/packaging.md` | Titles, description, chapters, tags, pinned comment, social posts | When it's writing those |
| `references/thumbnails.md` | Research, faces and cutouts, layout, rendering, the feed check, A/B tests | When it's making thumbnails |
| `scripts/thumbs.py` | Channel research, HTML to PNG renders, the feed-size check | Run with `uv run` |
| `evals/` | Tasks for `bench/agents`, with and without the behaviour | When measuring it |

## Install

On the machine (the owner's OK is the install, as for any extension):

```
mb ext add ~/path/to/mb-youtube
```

Then switch it on for the agents that publish: in the apps, or `behaviors = ["youtube"]` on the agent
in `wake.toml`. The node adds `SKILL.md` to each wake prompt for those agents only.

It's also an ordinary skill: copy the folder to `~/.claude/skills/youtube/`, and Claude Code loads it
when a task is about a YouTube upload.

## Requirements

`uv`, `yt-dlp`, `ffmpeg` and ImageMagick on the machine. Thumbnails render with Playwright's Chromium
(`uv run --with playwright playwright install chromium`), or with the Open Design extension where the
node has it.

## Sources

YouTube's own help pages for the limits and rules ([A/B tests](https://support.google.com/youtube/answer/16391400),
[chapters](https://support.google.com/youtube/answer/9884579), [hashtags](https://support.google.com/youtube/answer/6390658),
[tags](https://support.google.com/youtube/answer/146402)); the rest is cited where it's used.

MIT licence.
