---
name: youtube
description: How to research, write and design a YouTube upload's packaging (title, thumbnail, description, chapters, tags) on Pointman, for any channel, and how to upload it and change a video's details on the channel with the youtube commands. Use it when a task is any of those.
---

# YouTube publishing

For tasks on a video's titles, thumbnails, description, chapters, tags or social posts, and for
uploading it or changing a video on the channel. Skip it otherwise. `references/` and `scripts/` are in this behaviour's folder; your work (`research/`,
`publish/`) goes in the video's folder.

**The channel file.** `channel.md` lives in the channel's own folder (make it if it's missing). It
holds the brand (thumbnail formula, fonts, colours, whose face), the links footer and disclosures,
the tone, title patterns that work, and what the owner picked or rejected, and why. Read it first,
and add to it whenever the owner tells you something, so nobody asks twice.

**The order.**
1. **Research before drafting.** Learn the channel's past uploads and views, the niche's top videos,
   and this video's transcript. Promise only what the video delivers.
2. **Title and thumbnail are a pair.** The thumbnail adds what the title can't, and never repeats it.
3. **Real choices.** Make 2–3 genuinely different options, each title paired with a thumbnail. Post
   them as a decision (options and your pick, each with its evidence). The owner chooses.
4. **Revise at the same path,** so each round is a new version of the same card. Notes pinned on a
   card (`get_file`) are the change list.
5. **Report from evidence:** counts, checks, sources, and what you didn't check.

**Thumbnails:** read `references/thumbnails.md` first; the tools are in `scripts/thumbs.py`. In
short: one idea, 1–3 words readable at 168×94, a real frame of the person from this video (never a
generated likeness, and no effects on them unless asked), the bottom-right corner left clear for the
duration badge, and at least 1280×720. Check each one at feed size beside the channel's past
thumbnails before anyone sees it.

**Titles, description, chapters, tags:** read `references/packaging.md` first. In short: the
searched name in a title's first ~45 characters, chapters from the final cut (not the script),
every number and link checked against its source, and one paste-ready `publish/description.md`.

**On the channel:** the `youtube` commands, on the machine with the channel signed in: `create_job`
(or `/youtube <verb>` in a thread). `video` is the 11-character id; `file` is a board file
(`pointman://asset/<id>`) or a path on that machine.

| For | Command |
|---|---|
| What's on the channel: ids, titles, privacy, views, newest first | `youtube.videos {page_size?, page?}` |
| One video's title, description, tags, category, privacy, schedule | `youtube.video {video}` |
| A new video, private unless said | `youtube.upload {file, title, description, tags, category, privacy, publish_at?, made_for_kids?, thumbnail?, playlist?, confirm}` |
| Change its details: only the fields given change | `youtube.update {video, title?, description?, tags?, category?, privacy?, publish_at?, made_for_kids?, confirm?}` |
| Its thumbnail (JPEG or PNG, at most 2 MB) | `youtube.thumbnail {video, file, confirm?}` |

- **Ask the owner first, every time.** Before an upload, or any change to a public video (making one
  public or scheduling it counts), post exactly what will go up or change as a decision. Pass
  `confirm: true` only after their yes, for what they said yes to. Never set it on your own.
- Upload private (the default) and let the owner publish, or schedule it with `publish_at` once they
  pick the time.
- `description` and `tags` replace the whole field: read it with `youtube.video`, edit, send it all.
- The upload's `thumbnail` is a path on that machine. For a board file, run `youtube.thumbnail` after.
- Not signed in? Ask the owner to run `/youtube client` (once) and `/youtube add` in the app. Never
  ask for, paste or post a token. Uploads have a daily quota: don't retry a failed one in a loop.

**Stop and ask** before using a face, logo or footage the channel doesn't own.
