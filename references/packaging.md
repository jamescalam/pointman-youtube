# Packaging: titles, description, chapters, tags

Everything around the video except the thumbnail (that's [thumbnails.md](thumbnails.md)). Titles and
thumbnails are designed together: each title gets a thumbnail that adds to it, never repeats it.

The deliverable is one file, `publish/description.md`, with sections in the order YouTube Studio asks
for them, each ready to paste. Share it in the thread with `share_file`, and share it again at the same
path after every change. People read the card, not the Mac, so an unshared edit doesn't exist for them.

## 1. Read first

Gather these before writing anything. Each item says where its facts come from.

| What | Where | Why |
|---|---|---|
| The final cut | the export, or the edit's timeline | Chapters and timestamps come from this, never from the script |
| A transcript of the final cut | captions from the export, or speech-to-text (Whisper) on it | The words the video actually says. Captions from an earlier cut won't line up: say so, don't reuse them |
| The source | the script, article, repo or notes the video is built on | Every number and claim in the description comes from here |
| `channel.md` | the channel's own folder (see `SKILL.md`) | Links footer, socials, disclosures, tone, title patterns that work, past picks |
| The owner's past uploads | the channel's top videos (see 2) | What this audience clicks on |

If `channel.md` is missing something you need (the footer links, a sponsor disclosure), ask the owner
once in the thread, and write the answer into `channel.md` so nobody asks again.

## 2. Research

The goal is a title angle that is searched for, isn't already taken, and that the video delivers on.
Do it even when the request doesn't mention research: titles written without it are guesses. Spend
about 10 minutes. If `research/channel/` and `research/niche/` already exist, use them.

**Deliver, before writing any title:** `research/NOTES.md` (shared with the thumbnail research): the
names people search, the niche's top videos with views and what they promise, the open angle, and
links.

**What people call it.** List the names people search: the product, its maker, the problem it solves,
common misspellings. Sources:
- YouTube's suggestions: `curl "https://suggestqueries.google.com/complete/search?client=firefox&ds=yt&hl=en&q=<term>%20"`.
  Good for broad terms; for niche names it returns noise.
- Web search, to see what the docs, launch posts and forums call it.

**Who already covers it.** Search YouTube for the topic and read the results as data:
`uv run scripts/thumbs.py channel "ytsearch30:<topic>" --out research/niche` writes `videos.tsv`
(views, date, title) and a sheet of their thumbnails. Note what the top videos promise, how old they
are, and which angle has no strong video yet. That gap is your best title. Example: a big channel
already owned "X explained", but no one had "the open-source model that replaces X", which the video
showed and could back up.

**What works on this channel.** The owner's most-viewed uploads, and their titles' patterns:
`uv run scripts/thumbs.py channel https://www.youtube.com/@<handle>/videos --n 40 --out research/channel`.
Compare like with like: views depend on age and topic as much as on the title.

**Scores you can't fetch.** Keyword tools (vidIQ, TubeBuddy) and YouTube Studio's own Analytics →
Research tab need the owner's sign-in. If they use one, give them your candidate titles and tags to
score, and use their screenshot as evidence. Don't guess scores.

## 3. Titles

Write three, each making a different promise the video keeps. YouTube's Test & Compare tests up to
three titles, thumbnails, or pairs of both, and picks the winner by **watch time per impression**, not by
clicks. A title that overpromises gets the click, then loses the test.
([YouTube Help](https://support.google.com/youtube/answer/16391400))

- **Keyword early.** The searched name goes in the first ~45 characters. Phones cut titles at ~50, and
  most viewing is on phones.
- **Short.** Aim for 40–60 characters (the limit is 100).
- **True.** Every claim in the title is shown in the video. Name the timestamp that delivers it.
- **Three angles,** for example: the explainer (keyword-first), the comparison (X vs Y), the hands-on
  result (I tested…). Say for each which part of the video it sells, and the risk (e.g. "the open-source
  part starts at 16:39, so these viewers may skip ahead").
- **No `<` or `>`.** YouTube rejects them in titles, descriptions and tags.

Post them as a decision for the owner: options, your pick, and the evidence behind it (search results,
scores) in a line each. Pair each title with its thumbnail when thumbnails are ready.

## 4. Description

The first ~150 characters show in search and above "more", so they carry the keyword and what the video
is. Never open with "In this video". Then:

1. **Hook (1–2 sentences):** what it is, with the keyword as people search it, and why it matters.
2. **Summary (2–4 sentences):** what the video covers, in order.
3. **The main link** (article, repo, product), if there is one.
4. **Key results:** 3–5 bullets, each a number or finding from the source. Copy numbers from the
   source, never from memory, and keep their units and conditions ("~90 ms per call on the server").
5. **Chapters** (see 5).
6. **Links:** everything the video names, each with a short label. Fetch every URL to check it's live
   and is the page you mean.
7. **The channel's footer** from the channel file: socials, newsletter, disclosures.
8. **1–3 hashtags** at the very end. YouTube shows up to three above the title, and ignores all of them
   if there are more than 60. ([YouTube Help](https://support.google.com/youtube/answer/6390658))

Usually 200–500 words. The limit is 5,000 characters.

## 5. Chapters

Take them from the final cut: the edit's chapter cards or timeline markers, or topic changes in the
transcript of the export. Write down which timeline or export they came from and when, because any later
cut moves them.

YouTube's rules ([YouTube Help](https://support.google.com/youtube/answer/9884579)):
- the first is `0:00`;
- at least three, in ascending order;
- each at least 10 seconds long.

Check each one against the export: the transcript (or a frame) at that time should start that topic.
Names are short and searchable ("Open-source alternatives", not "Part 4").

## 6. Tags

YouTube says tags play a minimal role, mostly for names people misspell
([YouTube Help](https://support.google.com/youtube/answer/146402)), so spend little time on them. Use
the exact names (product, maker, people, package names), common misspellings, and the main topic terms.
Put them in priority order, so the owner can drop from the end.

The field holds 500 characters, counted YouTube's way: each tag's length, plus 2 for the quotes YouTube
adds to a tag with a space, plus 1 per comma. Check it before you post:

```python
tags = [t.strip() for t in open("tags.txt").read().split(",")]
print(sum(len(t) + 2 * (" " in t) for t in tags) + len(tags) - 1, "/ 500")
```

## 7. The rest of the pack

- **Pinned comment:** a question about the video's central tension that viewers can answer, plus the
  main link.
- **Captions:** from the final export. Flag any caption file that predates it.
- **Upload settings:** category, language, made for kids (no, unless it is), end screen, and the Test &
  Compare set (titles, thumbnails, or pairs). Test & Compare doesn't work on Shorts, Premieres, or videos
  that are private, made for kids or age-restricted.
- **Social posts, if asked:** written from the same source and numbers. On X, each post ≤280 characters
  (a link counts as 23). Leave `VIDEO_LINK` where the YouTube link goes, and say so.

## 8. Before you report

- Titles: count the characters. Keyword in the first ~45.
- Description: no `<` or `>`. Every number traces to the source. Every link was fetched.
- Chapters: `0:00` first, ≥3, each ≥10 s, checked against the export, with their source noted.
- Tags: ≤500 by the count above.
- The file is shared at its path, and the post says what you didn't check (e.g. "chapters are from the
  timeline as of 14:05; re-check if the cut changes").
