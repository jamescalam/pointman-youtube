# Thumbnails

Read this before designing a thumbnail. The tools are in `scripts/thumbs.py`; run it with
`uv run scripts/thumbs.py --help`. The first time, also run
`uv run --with playwright playwright install chromium`.

The order is: research, ask, frames, design, check at feed size, post for a pick, revise, record.
Each step below says what to deliver.

## 1. Research (about 20 min, before any design)

- **The channel's own history.** Run
  `uv run scripts/thumbs.py channel https://www.youtube.com/@<handle>/videos --n 40 --out research/channel`.
  It writes `videos.tsv` (id, views, date, title, by views), each thumbnail, and `sheet.jpg` labelled
  with views. It takes about 5–8 s a video. Note the formula: layout, font, where the face goes,
  colours, and any recurring device (a highlight box, a logo, a series badge). Note which videos did
  best against the channel's median. Views follow the topic more than the design, so compare like
  with like.
- **The niche.** Run `thumbs.py channel "ytsearch30:<the video's topic>" --out research/niche`.
  That's what this video will sit beside in search and suggestions. Find where it can stand out
  (colour, layout, an idea nobody uses), rather than copying the top result.
- **The video itself.** Read the transcript. Find the hook, the one result or moment worth showing,
  and the promise the title makes. A thumbnail may only promise what the video delivers.
- **Deliver:** `research/NOTES.md`, with the formula, what worked, the niche, and the outside
  guidance you used, with links. Share the two sheets.

## 2. Ask the owner, once, early, only what isn't recorded

Check the channel file (`channel.md`, see `SKILL.md`) first. Then, in one post, ask whatever is
missing, and write the answers into the channel file:
- the brand: fonts, colours, and a formula they want kept;
- whose face goes on it, if anyone's;
- what they dislike (effects on their face, expressions, styles).

Don't wait idle for the answer: say your assumptions, and carry on with research and frames.

## 3. Design rules

These are defaults. The channel's own numbers and the owner's taste come first.

- **One idea.** Show one focal object besides the face: a number, a word, a product, or a before/after.
- **1–3 words**, readable at 168×94. Text that only works at full size isn't doing anything.
- **Add to the title; don't repeat it.** The thumbnail carries what the title can't: a result, a
  contrast, a number, or a question. Design them as a pair, each thumbnail with its title.
- **Contrast:** light on dark (or the reverse), one accent colour, heavy type. No thin strokes, and
  no small text that matters.
- **Faces.** A real frame of the person from this video's own recording, in the same clothes and
  light as the video. A head about 40–50% of the frame's height, eyes in the upper third, looking at
  the lens or reacting. For explainers, curious, sceptical or engaged faces usually fit better than
  open-mouthed shock. Show the owner a face sheet and let them choose. Ask before adding glows,
  outlines or retouching to a person: many creators reject them.
- **Keep the corners clear.** The duration badge covers the bottom right (about 110×50 at 1280×720).
  A watched video shows a red progress bar along the bottom edge, and desktop shows hover icons at
  the top right. Keep text and faces out of all three.
- **Honest.** A misleading thumbnail loses watch time, and watch time is what YouTube's own tests
  score on.
- **Never** generate a likeness of a real person, and don't use logos, footage or faces the channel
  has no rights to without asking.

Sources:
- YouTube's requirements: https://support.google.com/youtube/answer/72431
- Test & compare: https://support.google.com/youtube/answer/16391400
- Industry heuristics (vendor blogs, so treat their figures as rough):
  https://vidiq.com/blog/post/youtube-thumbnail-tips/ and
  https://thumbnailtest.com/guides/youtube-thumbnail-examples

## 4. Frames and cutouts

- **Frames:** `ffmpeg -ss <t> -i <recording> -frames:v 1 -pix_fmt rgba frame.png`. Footage with
  alpha (ProRes 4444) decodes to 16-bit; `-pix_fmt rgba` gives a correct 8-bit PNG.
- **Face sheet:** take a frame every few seconds across the lively parts, crop to the face, and
  label each with its timestamp:
  `magick montage -label '%t' crops/*.png -tile 6x -geometry +6+6 faces.jpg`. Share it, and let the
  owner pick or veto.
- **Cutouts:** use the edit's own matte if there is one (an alpha export, or a keyed or matted
  clip). Otherwise use a matting model (for example `rembg`, or a SAM-based tool), or ask the agent
  that owns cutouts. Check the edges at 100%: hair halos show on dark backgrounds.
- **Backgrounds:** use the video's own set or B-roll, darkened or blurred so the face and words
  stand out.

## 5. Make it

- **HTML, rendered to PNG.** This always works, and it's what revisions are made in.
  - Make one page per concept: `html, body` at 1280×720, `overflow: hidden`, fonts from local files
    with `@font-face`, and assets by relative path.
  - Keep a `DESIGN.md` beside the pages: the artboard, and this video's palette, type and assets,
    taken from the channel file's brand. It's also what Open Design takes as `design_md`.
  - Render with `uv run scripts/thumbs.py render a=src/a.html b=src/b.html --out .`. That writes
    `a@2x.png` (2560×1440, the one to upload: YouTube takes up to 3840×2160) and `a.png`
    (1280×720), and a JPG when the PNG is over 2 MB.
- **Open Design**, if `opendesign_make` is in your tools. It designs a page from a brief plus a
  `design_md`, in about 15 min. It's good for exploring layouts. It can't read local images, so
  treat its result as a layout: move it into your own page with the real assets, and render that.
  Its still isn't 1280×720.

## 6. Check before anyone sees it

- **Feed check:** run `uv run scripts/thumbs.py feed a.png b.png --refs research/channel/<id>.jpg ...`.
  It shows each concept at 320×180 (the phone feed) and 168×94 (desktop "up next"), with rounded
  corners and a duration badge, beside the channel's past thumbnails. Open it and look:
  - Can you read every word at 168×94?
  - Is the idea clear in a second?
  - Does it stand out from its neighbours?
  - Is anything under the badge?
  Fix what fails before you post.
- **Files:** 16:9, at least 1280×720, and under 2 MB if the owner uploads from the phone app (the
  desktop limit is 50 MB).
- Open the full-size PNG too: check the edges, the crop, and any text running off the frame.

## 7. Deliver on the board

- Post 2–3 concepts that are genuinely different, not three tweaks of one. Use
  `share_file(path, thread_id=...)` for each, plus the feed check. Pair each with its title
  (A + 1, B + 2).
- Ask the owner to choose: `post_message(..., options=[...], pick=n)`, with one line on why you'd
  pick that one.
- **Revisions:** render to the same path and share again, so it's a new version of the same card.
  Notes pinned on a card (`get_asset(<id>)`) are the change list: answer each. For a redesign rather
  than a tweak, keep the old file as `<name>-v1.png` so they can compare.
- **After the pick,** suggest testing the top 2–3 in YouTube's Test & Compare. It's in Studio on
  desktop, takes up to 3 thumbnails, titles or pairs, and picks the winner by watch time, not CTR.
  It takes a few days to two weeks. Uploading and publishing are the owner's to do: don't.

## 8. Record

Write down what the owner chose and rejected, and why, in the channel file. The next video starts
from there, not from scratch.
