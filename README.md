# discourse-topic-render

Generate an **offline**, Discourse-like HTML page from a Discourse `topic.json` (with `cooked`) + CSS.

You can either:
- provide one or more local CSS files via `--css`, or
- omit `--css` and let the tool fetch `--base-url` and auto-discover stylesheet links.
- use a built-in minimal theme via `--builtin-css` (no CSS crawling).

## What it does (v1)

- Reads a user-provided `topic.json` (no automatic topic crawling).
- Downloads only the assets needed to render offline:
  - avatars
  - images referenced by `cooked`
  - CSS `@import` and `url(...)` dependencies (including Google Fonts CSS / woff2)
- Rewrites auto-loading URLs so the output opens without network.
- Keeps clickable links:
  - in-topic post links → rewritten to local `#post_<n>` anchors
  - other links remain clickable (site-relative links become absolute)
- Removes `<iframe>` and replaces it with a plain link.
- Does **not** download non-image attachments (keeps the link).

## Build

`cargo build --release`

## Usage

Directory mode (HTML + `assets/`):

`./target/release/discourse-topic-render --input topic.json --base-url https://forum.example.com --css site.css --mode dir --out out`

Single-file mode (everything inlined as `data:`):

`./target/release/discourse-topic-render --input topic.json --base-url https://forum.example.com --css site.css --mode single --out topic-123.html`

Auto-discover CSS from the site:

`./target/release/discourse-topic-render --input topic.json --base-url https://forum.example.com --mode dir --out out`

Use built-in minimal theme (light/dark):

`./target/release/discourse-topic-render --input topic.json --base-url https://forum.example.com --builtin-css --mode dir --out out`

## Progress UI

By default, the tool shows a progress UI when stderr is a TTY (`--progress auto`).

- Disable it (useful for CI / piping): `--progress never`
- Force-enable it: `--progress always`

## Notes on `topic.json`

This tool expects `post_stream.posts[].cooked` to be present for all posts you want to render.
Depending on how you export, a topic endpoint may only include the first chunk of posts; make sure you export a full JSON.

## Helper: merge paginated JSON

If you export multiple `...page=N...json` files, you can merge them into one:

`python3 tools/merge_topic_json_pages.py -o topic-merged.json topic-page1.json topic-page2.json`

## Helper: Tampermonkey export full `topic.json`

`tools/discourse_topic_json_exporter.user.js` is a Tampermonkey userscript that adds a small “导出 topic.json” panel on Discourse topic pages.
Click “开始”, and it will crawl the topic JSON in chunks at a configurable interval, then download a single merged `topic-<id>-<slug>-<timestamp>.json`.

Notes:
- “用 posts.json（推荐）” uses `/t/<topic_id>/posts.json?post_ids[]=...` (fewer requests). If it fails, it falls back to crawling `/<post_number>.json` pages.
- If you hit rate limits (`HTTP 429`), increase the request interval.
