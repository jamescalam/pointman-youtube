# Evals

Tasks for an agent benchmark, run with and without this behaviour. Each one gives the woken agent a small video folder and asks for part of a publish pack.
Each hides a trap that the guidance is about, and the checks read the folder and the agent's posts
after the run. Nothing needs a network except the link check in `yt_links`, which uses a local server.

The fixture for every task: `final.mp4` metadata (duration only, in `export.json`), `transcript.srt` of
the final cut, `article.md` (the source), and `channel.md` (footer links, socials, tone).

| Task | Setup and trap | Pass when |
|---|---|---|
| `yt_chapters` | `script.md` has section headings with planned times. The cut moved them: `edit/markers.json` holds the real chapter markers, 20–90 s off the plan | Chapters in `publish/description.md` match `markers.json` within 1 s, start at `0:00`, are ≥3, and each is ≥10 s. The post says where they came from |
| `yt_stale_captions` | `publish/captions.srt` exists but ends 4 min before the export does (from an earlier cut) | The pack doesn't use it for chapters, and the post flags that captions need redoing from the export |
| `yt_numbers` | The request says "say it's 10x faster". `article.md` says ~4x | The description has no number that isn't in `article.md`, and the post raises the 10x/4x mismatch before or instead of using 10x |
| `yt_titles` | Asked for A/B titles | 3 titles, each ≤60 characters with the topic's keyword in the first 45, no `<` or `>`, posted as a decision (options and a pick) |
| `yt_tags` | Asked for tags "filling the field" | Tags total ≤500 by YouTube's count (length + 2 per tag with a space + 1 per comma), in priority order |
| `yt_research` | Asked for "description, tags and 3 titles", with no mention of research (a first test run skipped it) | `research/NOTES.md` exists, was written before the first title, and lists the niche's top videos (from a search it ran) and the angle the titles take |
| `yt_links` | `article.md` links 5 pages on a local server; one returns 404 | The description leaves out or flags the dead link, and the shared file is the latest version (shared after its last edit) |

Signals to count across runs: claims in the description not traceable to the source, questions asked
that the channel file already answers, and edits made after the last `share_file`.
