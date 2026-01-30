// ==UserScript==
// @name         Discourse Topic JSON Exporter (Merged)
// @namespace    discourse-topic-render
// @version      0.1.1
// @description  Crawl a Discourse topic's paginated .json and download a merged topic.json for offline rendering.
// @match        *://*/*
// @grant        GM_addStyle
// ==/UserScript==

(() => {
  "use strict";

  const DEFAULTS = {
    requestIntervalMs: 800,
    postIdsChunkSize: 50,
    usePostsEndpoint: true,
    prettyJson: true,
    includeRaw: false,
  };

  const PANEL_ID = "dtr-topic-json-exporter";

  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  const nowIsoSafe = () =>
    new Date().toISOString().replace(/[:.]/g, "-").replace("T", "_").replace("Z", "Z");

  const safeFilename = (s) =>
    String(s)
      .trim()
      .replace(/[\\/:*?"<>|]+/g, "_")
      .replace(/\s+/g, "_")
      .replace(/_+/g, "_")
      .slice(0, 120) || "topic";

  const isDiscourse = () => {
    const gen = document.querySelector('meta[name="generator"]')?.getAttribute("content");
    if (typeof gen === "string" && gen.toLowerCase().includes("discourse")) return true;
    return typeof window.Discourse === "object" && window.Discourse !== null;
  };

  const parseTopicFromUrl = (url) => {
    const u = new URL(url);
    const idx = u.pathname.indexOf("/t/");
    if (idx < 0) return null;

    const prefix = u.pathname.slice(0, idx); // "" or "/forum" etc
    const rest = u.pathname.slice(idx + 3); // after "/t/"
    const parts = rest.split("/").filter(Boolean);
    if (parts.length < 1) return null;

    let slug = null;
    let topicId = null;
    let postNumber = null;

    if (/^\d+$/.test(parts[0])) {
      topicId = Number(parts[0]);
      if (parts.length >= 2 && /^\d+$/.test(parts[1])) postNumber = Number(parts[1]);
    } else if (parts.length >= 2 && /^\d+$/.test(parts[1])) {
      slug = parts[0];
      topicId = Number(parts[1]);
      if (parts.length >= 3 && /^\d+$/.test(parts[2])) postNumber = Number(parts[2]);
    } else {
      return null;
    }

    return { origin: u.origin, prefix, slug, topicId, postNumber };
  };

  const getCanonicalUrl = () => {
    const canonical =
      document.querySelector('link[rel="canonical"]')?.getAttribute("href") ||
      document.querySelector('meta[property="og:url"]')?.getAttribute("content");
    if (canonical) return canonical;
    return window.location.href;
  };

  const topicBasePath = ({ prefix, slug, topicId }) =>
    `${prefix}/t/${slug ? `${slug}/` : ""}${topicId}`;

  const buildTopicJsonUrl = (topic, startPostNumber, includeRaw) => {
    const path =
      typeof startPostNumber === "number" && startPostNumber > 1
        ? `${topicBasePath(topic)}/${startPostNumber}.json`
        : `${topicBasePath(topic)}.json`;

    const u = new URL(path, topic.origin);
    if (includeRaw) u.searchParams.set("include_raw", "1");
    return u.toString();
  };

  const buildPostsByIdsUrl = (topic, postIds, includeRaw) => {
    const u = new URL(`${topic.prefix}/t/${topic.topicId}/posts.json`, topic.origin);
    for (const id of postIds) u.searchParams.append("post_ids[]", String(id));
    if (includeRaw) u.searchParams.set("include_raw", "1");
    return u.toString();
  };

  const fetchJson = async (url, { signal, maxRetries = 6 } = {}) => {
    for (let attempt = 0; ; attempt++) {
      const res = await fetch(url, {
        method: "GET",
        credentials: "same-origin",
        signal,
        headers: { Accept: "application/json" },
      });

      if (res.status === 429 && attempt < maxRetries) {
        const ra = res.headers.get("Retry-After");
        const waitMs = ra && /^\d+$/.test(ra) ? Number(ra) * 1000 : Math.min(30000, 500 * 2 ** attempt);
        await sleep(waitMs);
        continue;
      }

      if (!res.ok) {
        const snippet = await res.text().catch(() => "");
        throw new Error(`HTTP ${res.status} ${res.statusText} (${url})${snippet ? `: ${snippet.slice(0, 200)}` : ""}`);
      }

      try {
        return await res.json();
      } catch (e) {
        const snippet = await res.text().catch(() => "");
        throw new Error(`Non-JSON response (${url})${snippet ? `: ${snippet.slice(0, 200)}` : ""}`);
      }
    }
  };

  const findPosts = (doc) => {
    if (doc && typeof doc === "object") {
      if (doc.post_stream && typeof doc.post_stream === "object" && Array.isArray(doc.post_stream.posts)) {
        return { posts: doc.post_stream.posts, path: ["post_stream", "posts"] };
      }
      if (Array.isArray(doc.posts)) return { posts: doc.posts, path: ["posts"] };
    }
    throw new Error("JSON 不包含 `post_stream.posts`（或顶层 `posts`）");
  };

  const topicIdFromDoc = (doc) => {
    if (doc && typeof doc === "object") {
      if (Number.isInteger(doc.id)) return doc.id;
      if (Number.isInteger(doc.topic_id)) return doc.topic_id;
    }
    return null;
  };

  const postKey = (post) => {
    if (!post || typeof post !== "object") return null;
    if (Number.isInteger(post.id)) return ["id", post.id];
    if (Number.isInteger(post.post_number)) return ["post_number", post.post_number];
    if (typeof post.username === "string" && typeof post.created_at === "string") {
      return ["username+created_at", `${post.username}\n${post.created_at}`];
    }
    return null;
  };

  const postSortKey = (post) => {
    if (post && typeof post === "object") {
      if (Number.isInteger(post.post_number)) return [0, post.post_number];
      if (Number.isInteger(post.id)) return [1, post.id];
    }
    return [2, 0];
  };

  const userKey = (user) => {
    if (!user || typeof user !== "object") return null;
    if (Number.isInteger(user.id)) return ["id", user.id];
    if (typeof user.username === "string") return ["username", user.username];
    return null;
  };

  const setPath = (doc, path, value) => {
    let cur = doc;
    for (let i = 0; i < path.length - 1; i++) {
      const key = path[i];
      if (!cur || typeof cur !== "object" || !(key in cur)) throw new Error(`无法写入路径：${path.join(".")}`);
      cur = cur[key];
    }
    const last = path[path.length - 1];
    if (!cur || typeof cur !== "object") throw new Error(`无法写入路径：${path.join(".")}`);
    cur[last] = value;
  };

  const setPostStreamStream = (base, posts) => {
    if (!base || typeof base !== "object") return;
    if (!base.post_stream || typeof base.post_stream !== "object") return;
    const ids = [];
    for (const p of posts) {
      if (p && typeof p === "object" && Number.isInteger(p.id)) ids.push(p.id);
    }
    if (ids.length) base.post_stream.stream = ids;
  };

  const downloadBlob = (blob, filename) => {
    // Tampermonkey's GM_download may fail on `blob:` URLs ("Check internet connection").
    // Use a plain <a download> so it goes through the browser's download flow.
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.rel = "noopener noreferrer";
    document.body.appendChild(a);
    a.click();
    a.remove();
    // Keep the object URL alive for a while in case the browser delays the download.
    setTimeout(() => URL.revokeObjectURL(url), 5 * 60_000);
  };

  const ensurePanel = () => {
    if (document.getElementById(PANEL_ID)) return;
    if (!isDiscourse()) return;

    const topic = parseTopicFromUrl(getCanonicalUrl());
    if (!topic) return;

    GM_addStyle?.(`
      #${PANEL_ID} {
        position: fixed;
        right: 14px;
        bottom: 14px;
        z-index: 2147483647;
        width: 260px;
        padding: 10px;
        border-radius: 10px;
        background: rgba(20, 20, 22, 0.92);
        color: #fff;
        font: 12px/1.35 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol";
        box-shadow: 0 10px 30px rgba(0, 0, 0, 0.35);
        backdrop-filter: blur(8px);
      }
      #${PANEL_ID} * { box-sizing: border-box; }
      #${PANEL_ID} .dtr-row { margin-top: 8px; }
      #${PANEL_ID} .dtr-title { display:flex; align-items:center; justify-content:space-between; font-weight: 700; }
      #${PANEL_ID} .dtr-kv { display:flex; gap:8px; align-items:center; }
      #${PANEL_ID} input[type="number"] {
        width: 100%;
        padding: 6px 8px;
        border-radius: 8px;
        border: 1px solid rgba(255,255,255,.18);
        background: rgba(255,255,255,.08);
        color: #fff;
        outline: none;
      }
      #${PANEL_ID} label { user-select:none; }
      #${PANEL_ID} .dtr-actions { display:flex; gap:8px; }
      #${PANEL_ID} button {
        flex: 1;
        padding: 7px 8px;
        border-radius: 8px;
        border: 1px solid rgba(255,255,255,.18);
        background: rgba(255,255,255,.14);
        color: #fff;
        cursor: pointer;
      }
      #${PANEL_ID} button:disabled { opacity: 0.55; cursor: not-allowed; }
      #${PANEL_ID} .dtr-status {
        margin-top: 8px;
        padding: 6px 8px;
        border-radius: 8px;
        background: rgba(255,255,255,.08);
        white-space: pre-wrap;
        max-height: 140px;
        overflow: auto;
      }
      #${PANEL_ID} .dtr-small { opacity: 0.85; font-size: 11px; }
      #${PANEL_ID} .dtr-close {
        border: none;
        background: transparent;
        padding: 0;
        width: auto;
        cursor: pointer;
        opacity: 0.8;
      }
      #${PANEL_ID} .dtr-close:hover { opacity: 1; }
      #${PANEL_ID} .dtr-link {
        color: rgba(255,255,255,.9);
        text-decoration: none;
      }
      #${PANEL_ID} .dtr-link:hover { text-decoration: underline; }
    `);

    const root = document.createElement("div");
    root.id = PANEL_ID;

    root.innerHTML = `
      <div class="dtr-title">
        <div>导出 topic.json</div>
        <button class="dtr-close" title="隐藏">✕</button>
      </div>
      <div class="dtr-row dtr-small">
        话题：<a class="dtr-link" target="_blank" rel="noopener noreferrer"></a>
      </div>
      <div class="dtr-row dtr-kv">
        <label style="flex: 1">请求间隔 (ms)</label>
        <div style="flex: 1"><input class="dtr-interval" type="number" min="0" step="50"></div>
      </div>
      <div class="dtr-row dtr-kv">
        <label style="flex: 1">每次拉取 post_ids 数</label>
        <div style="flex: 1"><input class="dtr-chunk" type="number" min="1" max="200" step="1"></div>
      </div>
      <div class="dtr-row dtr-kv">
        <label style="flex: 1"><input class="dtr-use-posts" type="checkbox"> 用 posts.json（推荐）</label>
      </div>
      <div class="dtr-row dtr-kv">
        <label style="flex: 1"><input class="dtr-pretty" type="checkbox"> 美化 JSON（更大）</label>
      </div>
      <div class="dtr-row dtr-kv">
        <label style="flex: 1"><input class="dtr-raw" type="checkbox"> include_raw=1</label>
      </div>
      <div class="dtr-row dtr-actions">
        <button class="dtr-start">开始</button>
        <button class="dtr-cancel" disabled>取消</button>
      </div>
      <div class="dtr-status">就绪</div>
    `;

    const link = root.querySelector(".dtr-link");
    link.textContent = `${topic.topicId}${topic.slug ? ` / ${topic.slug}` : ""}`;
    link.href = `${topic.origin}${topicBasePath(topic)}`;

    const intervalInput = root.querySelector(".dtr-interval");
    intervalInput.value = String(DEFAULTS.requestIntervalMs);
    const chunkInput = root.querySelector(".dtr-chunk");
    chunkInput.value = String(DEFAULTS.postIdsChunkSize);
    const usePostsInput = root.querySelector(".dtr-use-posts");
    usePostsInput.checked = DEFAULTS.usePostsEndpoint;
    const prettyInput = root.querySelector(".dtr-pretty");
    prettyInput.checked = DEFAULTS.prettyJson;
    const rawInput = root.querySelector(".dtr-raw");
    rawInput.checked = DEFAULTS.includeRaw;

    const startBtn = root.querySelector(".dtr-start");
    const cancelBtn = root.querySelector(".dtr-cancel");
    const closeBtn = root.querySelector(".dtr-close");
    const statusEl = root.querySelector(".dtr-status");

    let running = false;
    let controller = null;

    const setStatus = (msg) => {
      statusEl.textContent = msg;
    };

    const stopUi = () => {
      running = false;
      controller = null;
      startBtn.disabled = false;
      cancelBtn.disabled = true;
    };

    closeBtn.addEventListener("click", () => {
      if (running && controller) controller.abort();
      root.remove();
    });

    cancelBtn.addEventListener("click", () => {
      if (controller) controller.abort();
    });

    startBtn.addEventListener("click", async () => {
      if (running) return;
      running = true;
      controller = new AbortController();
      startBtn.disabled = true;
      cancelBtn.disabled = false;

      const requestIntervalMs = Math.max(0, Number(intervalInput.value || DEFAULTS.requestIntervalMs));
      const postIdsChunkSize = Math.max(1, Math.min(200, Number(chunkInput.value || DEFAULTS.postIdsChunkSize)));
      const usePostsEndpoint = Boolean(usePostsInput.checked);
      const prettyJson = Boolean(prettyInput.checked);
      const includeRaw = Boolean(rawInput.checked);

      const canonicalTopic = parseTopicFromUrl(getCanonicalUrl());
      if (!canonicalTopic) {
        setStatus("无法解析话题 URL（需要 /t/.../...）");
        stopUi();
        return;
      }

      try {
        setStatus("拉取第一页 topic.json ...");
        const baseDoc = await fetchJson(buildTopicJsonUrl(canonicalTopic, 1, includeRaw), {
          signal: controller.signal,
        });

        const docTopicId = topicIdFromDoc(baseDoc);
        if (docTopicId !== null && docTopicId !== canonicalTopic.topicId) {
          throw new Error(`topic id 不匹配：URL=${canonicalTopic.topicId} JSON=${docTopicId}`);
        }

        const { posts: basePosts, path: postsPath } = findPosts(baseDoc);
        const chunkSize = Number.isInteger(baseDoc.chunk_size) ? baseDoc.chunk_size : 20;
        const streamIds = Array.isArray(baseDoc?.post_stream?.stream) ? baseDoc.post_stream.stream : null;
        const expectedPosts = streamIds && streamIds.every((x) => Number.isInteger(x)) ? streamIds.length : null;

        const postsSeen = new Map();
        const mergedPosts = [];

        const addPosts = (posts) => {
          if (!Array.isArray(posts)) return 0;
          let added = 0;
          for (const p of posts) {
            if (!p || typeof p !== "object") continue;
            const k = postKey(p);
            if (!k) continue;
            const ks = `${k[0]}\n${String(k[1])}`;
            if (postsSeen.has(ks)) continue;
            postsSeen.set(ks, true);
            mergedPosts.push(p);
            added++;
          }
          return added;
        };

        const usersSeen = new Map();
        const mergedUsers = [];
        const trackUsers = Array.isArray(baseDoc.users);

        const addUsers = (users) => {
          if (!trackUsers) return 0;
          if (!Array.isArray(users)) return 0;
          let added = 0;
          for (const u of users) {
            if (!u || typeof u !== "object") continue;
            const k = userKey(u);
            if (!k) continue;
            const ks = `${k[0]}\n${String(k[1])}`;
            if (usersSeen.has(ks)) continue;
            usersSeen.set(ks, true);
            mergedUsers.push(u);
            added++;
          }
          return added;
        };

        addPosts(basePosts);
        addUsers(baseDoc.users);

        const progress = (extra) => {
          const total = expectedPosts ?? "?";
          setStatus(
            `进行中...\n` +
              `已合并 posts: ${mergedPosts.length}/${total}\n` +
              (trackUsers ? `已合并 users: ${mergedUsers.length}\n` : "") +
              (extra ? `${extra}\n` : "") +
              `chunk_size=${chunkSize}`
          );
        };

        progress();

        const tryPostsEndpoint = async () => {
          if (!usePostsEndpoint) return false;
          if (!expectedPosts || !streamIds) return false;

          const haveIds = new Set();
          for (const p of mergedPosts) {
            if (p && typeof p === "object" && Number.isInteger(p.id)) haveIds.add(p.id);
          }
          const missing = streamIds.filter((id) => Number.isInteger(id) && !haveIds.has(id));
          if (missing.length === 0) return true;

          let fetched = 0;
          for (let i = 0; i < missing.length; i += postIdsChunkSize) {
            if (controller.signal.aborted) throw new Error("已取消");
            const batch = missing.slice(i, i + postIdsChunkSize);
            const url = buildPostsByIdsUrl(canonicalTopic, batch, includeRaw);

            if (requestIntervalMs > 0) await sleep(requestIntervalMs);
            const doc = await fetchJson(url, { signal: controller.signal });

            const { posts } = findPosts(doc);
            const addedPosts = addPosts(posts);
            addUsers(doc.users);
            fetched += batch.length;
            progress(`posts.json: ${fetched}/${missing.length} ids（本次新增 posts: ${addedPosts}）`);
          }
          return true;
        };

        const doPageCrawl = async () => {
          const highest = Number.isInteger(baseDoc.highest_post_number) ? baseDoc.highest_post_number : null;
          const maxPages = 5000;
          let pageCount = 0;

          const maxStart =
            highest !== null ? highest : chunkSize * maxPages;

          for (let start = chunkSize + 1; start <= maxStart; start += chunkSize) {
            pageCount++;
            if (pageCount > maxPages) throw new Error(`超过最大分页数限制（${maxPages}）`);
            if (controller.signal.aborted) throw new Error("已取消");

            const url = buildTopicJsonUrl(canonicalTopic, start, includeRaw);
            if (requestIntervalMs > 0) await sleep(requestIntervalMs);

            let doc;
            try {
              doc = await fetchJson(url, { signal: controller.signal });
            } catch (e) {
              if (highest !== null && start > highest) break;
              throw e;
            }

            const { posts } = findPosts(doc);
            const addedPosts = addPosts(posts);
            addUsers(doc.users);
            progress(`分页: start=${start}（本次新增 posts: ${addedPosts}）`);

            if (posts.length === 0) break;
          }
        };

        let usedPostsEndpoint = false;
        try {
          usedPostsEndpoint = await tryPostsEndpoint();
        } catch (e) {
          progress(`posts.json 失败，回退分页模式: ${String(e?.message || e)}`);
        }

        if (!usedPostsEndpoint) await doPageCrawl();

        mergedPosts.sort((a, b) => {
          const ak = postSortKey(a);
          const bk = postSortKey(b);
          if (ak[0] !== bk[0]) return ak[0] - bk[0];
          return ak[1] - bk[1];
        });

        setPath(baseDoc, postsPath, mergedPosts);
        if (trackUsers) baseDoc.users = mergedUsers;
        setPostStreamStream(baseDoc, mergedPosts);

        const indent = prettyJson ? 2 : 0;
        const jsonText = JSON.stringify(baseDoc, null, indent) + "\n";
        const blob = new Blob([jsonText], { type: "application/json;charset=utf-8" });

        const slugPart = canonicalTopic.slug ? `-${safeFilename(canonicalTopic.slug)}` : "";
        const filename = safeFilename(`topic-${canonicalTopic.topicId}${slugPart}-${nowIsoSafe()}.json`);

        downloadBlob(blob, filename);
        setStatus(`完成 ✅\n已下载：${filename}\nposts=${mergedPosts.length}${trackUsers ? ` users=${mergedUsers.length}` : ""}`);
        stopUi();
      } catch (e) {
        setStatus(`失败 ❌\n${String(e?.message || e)}`);
        stopUi();
      }
    });

    document.body.appendChild(root);
  };

  const boot = () => {
    ensurePanel();
    setInterval(ensurePanel, 2000);
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot, { once: true });
  } else {
    boot();
  }
})();
