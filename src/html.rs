use anyhow::Context as _;
use kuchiki::traits::TendrilSink as _;
use maud::{DOCTYPE, Markup, PreEscaped, html};
use url::Url;

use crate::assets::{AssetKind, AssetRequest, AssetSource, AssetStore};
use crate::builtin;
use crate::topic::{Post, TopicJson};

pub struct RenderedPost {
    pub post_number: u64,
    pub username: String,
    pub created_at: Option<String>,
    pub avatar_src: String,
    pub cooked_html: String,
}

pub struct RenderContext<'a> {
    pub base_url: &'a Url,
    pub topic_id: u64,
}

pub async fn render_posts(
    topic: &TopicJson,
    base_url: &Url,
    avatar_size: u32,
    store: &AssetStore,
) -> anyhow::Result<Vec<RenderedPost>> {
    let mut rendered = Vec::with_capacity(topic.post_stream.posts.len());
    for post in &topic.post_stream.posts {
        let cooked = post.cooked.as_deref().unwrap_or("").trim().to_string();
        if cooked.is_empty() {
            continue;
        }

        let username = post
            .display_username
            .clone()
            .or_else(|| post.username.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let avatar_src = resolve_and_fetch_avatar(post, base_url, avatar_size, store).await?;

        let cooked_html = rewrite_cooked_html(
            &cooked,
            &RenderContext {
                base_url,
                topic_id: topic.id,
            },
            store,
        )
        .await
        .with_context(|| format!("rewrite cooked html for post {}", post.post_number))?;

        rendered.push(RenderedPost {
            post_number: post.post_number,
            username,
            created_at: post.created_at.clone(),
            avatar_src,
            cooked_html,
        });
    }
    Ok(rendered)
}

async fn resolve_and_fetch_avatar(
    post: &Post,
    base_url: &Url,
    avatar_size: u32,
    store: &AssetStore,
) -> anyhow::Result<String> {
    let template = post.avatar_template.as_deref().unwrap_or("");
    if template.is_empty() {
        return Ok(String::new());
    }

    let mut t = template.to_string();
    if t.contains("{size}") {
        t = t.replace("{size}", &avatar_size.to_string());
    }

    let url = resolve_any_url(base_url, &t)
        .with_context(|| format!("resolve avatar_template {}", template))?;
    let req = AssetRequest {
        kind: AssetKind::Avatar,
        source: AssetSource::Remote(url),
    };
    store.get(req).await
}

pub async fn rewrite_cooked_html(
    cooked: &str,
    ctx: &RenderContext<'_>,
    store: &AssetStore,
) -> anyhow::Result<String> {
    let document = kuchiki::parse_html().one(cooked);

    // Remove scripts entirely.
    if let Ok(nodes) = document.select("script") {
        for node in nodes {
            node.as_node().detach();
        }
    }

    // Replace iframes with plain links.
    if let Ok(nodes) = document.select("iframe") {
        for node in nodes {
            let href = node
                .attributes
                .borrow()
                .get("src")
                .map(|s| s.to_string())
                .unwrap_or_default();
            let link = make_link_node(&href);
            node.as_node().insert_before(link);
            node.as_node().detach();
        }
    }

    // Replace audio/video with link(s), do not download.
    for selector in ["audio", "video"] {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                let href = node
                    .attributes
                    .borrow()
                    .get("src")
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let link = make_link_node(&href);
                node.as_node().insert_before(link);
                node.as_node().detach();
            }
        }
    }

    // Rewrite <img>.
    if let Ok(nodes) = document.select("img") {
        for node in nodes {
            rewrite_img_like(node, ctx.base_url, store).await?;
        }
    }

    // Rewrite <source> inside picture/video/audio.
    if let Ok(nodes) = document.select("source") {
        for node in nodes {
            let mut attrs = node.attributes.borrow_mut();
            if let Some(srcset) = attrs.get("srcset").map(|s| s.to_string()) {
                if let Some(best) = choose_best_src_from_srcset(&srcset) {
                    let url = resolve_any_url(ctx.base_url, &best)?;
                    let req = AssetRequest {
                        kind: AssetKind::Image,
                        source: AssetSource::Remote(url),
                    };
                    let new_src = store.get(req).await?;
                    attrs.insert("src", new_src);
                    attrs.remove("srcset");
                }
            } else if let Some(src) = attrs.get("src").map(|s| s.to_string()) {
                if !src.trim().starts_with("data:") && !src.trim().is_empty() {
                    let url = resolve_any_url(ctx.base_url, &src)?;
                    let req = AssetRequest {
                        kind: AssetKind::Image,
                        source: AssetSource::Remote(url),
                    };
                    let new_src = store.get(req).await?;
                    attrs.insert("src", new_src);
                }
            }
        }
    }

    // Rewrite style="...url(...)..."
    if let Ok(nodes) = document.select("[style]") {
        for node in nodes {
            let style = node.attributes.borrow().get("style").map(|s| s.to_string());
            let Some(style) = style else { continue };
            let rewritten = rewrite_inline_style(&style, ctx.base_url, store).await?;
            node.attributes.borrow_mut().insert("style", rewritten);
        }
    }

    // Rewrite lightbox links if they look like image hrefs.
    if let Ok(nodes) = document.select("a.lightbox") {
        for node in nodes {
            let href = node.attributes.borrow().get("href").map(|s| s.to_string());
            let Some(href) = href else { continue };
            if !looks_like_image_url(&href) {
                continue;
            }
            let url = resolve_any_url(ctx.base_url, &href)?;
            let req = AssetRequest {
                kind: AssetKind::Image,
                source: AssetSource::Remote(url),
            };
            let new_href = store.get(req).await?;
            node.attributes.borrow_mut().insert("href", new_href);
        }
    }

    // Rewrite in-topic links to anchors.
    if let Ok(nodes) = document.select("a[href]") {
        for node in nodes {
            let href = node.attributes.borrow().get("href").map(|s| s.to_string());
            let Some(href) = href else { continue };
            if let Some(anchor) = topic_local_anchor(ctx.base_url, ctx.topic_id, &href) {
                node.attributes.borrow_mut().insert("href", anchor);
                continue;
            }
            if should_absolutize_href(&href) {
                if let Ok(url) = resolve_any_url(ctx.base_url, &href) {
                    node.attributes.borrow_mut().insert("href", url.to_string());
                }
            }
        }
    }

    // Serialize body children only (avoid wrapping <html><body> around cooked).
    let body = document
        .select_first("body")
        .ok()
        .map(|n| n.as_node().clone());

    let mut out = Vec::new();
    if let Some(body) = body {
        for child in body.children() {
            child
                .serialize(&mut out)
                .context("serialize cooked child")?;
        }
    } else {
        document.serialize(&mut out).context("serialize cooked")?;
    }
    Ok(String::from_utf8(out).context("cooked html not utf-8")?)
}

async fn rewrite_img_like(
    node: kuchiki::NodeDataRef<kuchiki::ElementData>,
    base_url: &Url,
    store: &AssetStore,
) -> anyhow::Result<()> {
    let mut attrs = node.attributes.borrow_mut();

    if let Some(srcset) = attrs.get("srcset").map(|s| s.to_string()) {
        if let Some(best) = choose_best_src_from_srcset(&srcset) {
            let url = resolve_any_url(base_url, &best)?;
            let req = AssetRequest {
                kind: AssetKind::Image,
                source: AssetSource::Remote(url),
            };
            let new_src = store.get(req).await?;
            attrs.insert("src", new_src);
            attrs.remove("srcset");
            return Ok(());
        }
    }

    if let Some(src) = attrs.get("src").map(|s| s.to_string()) {
        let s = src.trim();
        if s.is_empty() || s.starts_with("data:") {
            return Ok(());
        }
        let url = resolve_any_url(base_url, s)?;
        let req = AssetRequest {
            kind: AssetKind::Image,
            source: AssetSource::Remote(url),
        };
        let new_src = store.get(req).await?;
        attrs.insert("src", new_src);
    }

    Ok(())
}

pub fn build_html(
    topic: &TopicJson,
    posts: &[RenderedPost],
    css: &str,
    css_link_href: Option<&str>,
) -> String {
    let title = topic.title.as_str();
    let markup: Markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                @if let Some(href) = css_link_href {
                    link rel="stylesheet" href=(href);
                } @else {
                    style { (PreEscaped(css)) }
                }
            }
            body class="crawler" {
                div id="main-outlet" class="wrap" {
                    header class="topic-header" {
                        h1 class="topic-title" { (title) }
                    }
                    main class="topic-posts" {
                        @for p in posts {
                            (render_post(p))
                        }
                    }
                }
            }
        }
    };
    markup.into_string()
}

pub fn build_html_minimal(
    topic: &TopicJson,
    posts: &[RenderedPost],
    css: &str,
    css_link_href: Option<&str>,
) -> String {
    let title = topic.title.as_str();
    let post_count = posts.len();

    let markup: Markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) }
                @if let Some(href) = css_link_href {
                    link rel="stylesheet" href=(href);
                } @else {
                    style { (PreEscaped(css)) }
                }
            }
            body class="dtr" {
                header class="dtr-topbar" {
                    div class="dtr-container dtr-topbar-inner" {
                        div class="dtr-title" {
                            h1 { (title) }
                        }
                        button type="button" id="dtr-theme-toggle" class="dtr-btn" { "Theme" }
                    }
                }
                main class="dtr-container dtr-main" {
                    @for p in posts {
                        (render_post_minimal(p))
                    }
                }
                footer class="dtr-footer" {
                    div class="dtr-container" {
                        "Posts: " (post_count)
                    }
                }
                script { (PreEscaped(builtin::THEME_TOGGLE_JS)) }
            }
        }
    };
    markup.into_string()
}

fn render_post(p: &RenderedPost) -> Markup {
    let post_id = format!("post_{}", p.post_number);
    let post_number = p.post_number;
    let created_at = p.created_at.as_deref().unwrap_or("");

    html! {
        article id=(post_id) class="topic-post" {
            div class="post-wrapper" {
                aside class="topic-avatar" {
                    @if !p.avatar_src.is_empty() {
                        img class="avatar" width="45" height="45" src=(p.avatar_src) alt="avatar";
                    }
                }
                section class="topic-body" {
                    header class="topic-meta-data" {
                        div class="names" {
                            span class="username" { (p.username) }
                        }
                        div class="post-info" {
                            span class="post-number" { "#" (post_number) }
                            @if !created_at.is_empty() {
                                " "
                                time datetime=(created_at) { (created_at) }
                            }
                        }
                    }
                    div class="cooked" {
                        (PreEscaped(&p.cooked_html))
                    }
                }
            }
        }
    }
}

fn render_post_minimal(p: &RenderedPost) -> Markup {
    let post_id = format!("post_{}", p.post_number);
    let post_number = p.post_number;
    let created_at = p.created_at.as_deref().unwrap_or("");

    html! {
        article id=(post_id) class="dtr-post" {
            header class="dtr-post-header" {
                @if !p.avatar_src.is_empty() {
                    div class="dtr-post-avatar" {
                        img class="dtr-avatar" width="40" height="40" src=(p.avatar_src) alt="avatar";
                    }
                }
                div class="dtr-post-meta" {
                    div class="dtr-post-meta-top" {
                        span class="dtr-username" { (p.username) }
                    }
                    div class="dtr-post-sub" {
                        a class="dtr-post-number" href=(format!("#{}", post_id)) { "#" (post_number) }
                        @if !created_at.is_empty() {
                            time datetime=(created_at) { (created_at) }
                        }
                    }
                }
            }
            div class="cooked dtr-cooked" {
                (PreEscaped(&p.cooked_html))
            }
        }
    }
}

fn make_link_node(href: &str) -> kuchiki::NodeRef {
    let safe = href.trim();
    let display = if safe.is_empty() { "link" } else { safe };
    let frag = format!(
        "<p><a href=\"{}\" rel=\"noreferrer noopener\">{}</a></p>",
        html_escape_attr(safe),
        html_escape_text(display)
    );
    let doc = kuchiki::parse_html().one(frag);
    doc.select_first("a").unwrap().as_node().clone()
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn looks_like_image_url(href: &str) -> bool {
    let h = href.to_ascii_lowercase();
    ["png", "jpg", "jpeg", "gif", "webp", "svg", "avif"]
        .iter()
        .any(|ext| {
            h.split('?')
                .next()
                .unwrap_or("")
                .ends_with(&format!(".{ext}"))
        })
}

fn resolve_any_url(base_url: &Url, raw: &str) -> anyhow::Result<Url> {
    let r = raw.trim();
    if r.starts_with("http://") || r.starts_with("https://") {
        return Ok(Url::parse(r)?);
    }
    if r.starts_with("//") {
        return Ok(Url::parse(&format!("{}:{}", base_url.scheme(), r))?);
    }
    Ok(base_url.join(r)?)
}

fn should_absolutize_href(href: &str) -> bool {
    let h = href.trim();
    if h.is_empty()
        || h.starts_with('#')
        || h.starts_with("mailto:")
        || h.starts_with("tel:")
        || h.starts_with("javascript:")
        || h.starts_with("data:")
    {
        return false;
    }
    !(h.starts_with("http://") || h.starts_with("https://"))
}

fn choose_best_src_from_srcset(srcset: &str) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for part in srcset.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut pieces = part.split_whitespace();
        let url = pieces.next()?.to_string();
        let descriptor = pieces.next().unwrap_or("");
        let score = if descriptor.ends_with('w') || descriptor.ends_with('x') {
            descriptor[..descriptor.len().saturating_sub(1)]
                .parse::<f64>()
                .unwrap_or(0.0)
        } else {
            0.0
        };
        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best = Some((score, url)),
        }
    }
    best.map(|(_, url)| url)
}

fn topic_local_anchor(base_url: &Url, topic_id: u64, href: &str) -> Option<String> {
    // Accept absolute or relative URLs.
    let resolved = if href.starts_with("http://") || href.starts_with("https://") {
        Url::parse(href).ok()?
    } else if href.starts_with("//") {
        Url::parse(&format!("{}:{}", base_url.scheme(), href)).ok()?
    } else {
        base_url.join(href).ok()?
    };

    // Must be same host and /t/... structure.
    if resolved.host_str() != base_url.host_str() {
        return None;
    }

    // Fast path: already a post anchor.
    if let Some(fragment) = resolved.fragment() {
        if fragment.starts_with("post_") {
            return Some(format!("#{}", fragment));
        }
    }

    let segs: Vec<_> = resolved
        .path_segments()
        .map(|s| s.collect::<Vec<_>>())
        .unwrap_or_default();
    if segs.is_empty() || segs[0] != "t" {
        return None;
    }

    let (topic_seg, post_seg) = if segs.get(1).and_then(|s| s.parse::<u64>().ok()).is_some() {
        (segs.get(1)?, segs.get(2))
    } else {
        (segs.get(2)?, segs.get(3))
    };

    let topic = topic_seg.parse::<u64>().ok()?;
    if topic != topic_id {
        return None;
    }

    let post = post_seg?.parse::<u64>().ok()?;
    Some(format!("#post_{}", post))
}

async fn rewrite_inline_style(
    style: &str,
    base_url: &Url,
    store: &AssetStore,
) -> anyhow::Result<String> {
    let re = regex::Regex::new(
        r#"url\(\s*(?:(?:"(?P<u_d>[^"]+)"|'(?P<u_s>[^']+)'|(?P<u2>[^)]+)))\s*\)"#,
    )
    .expect("inline style url regex");
    let mut out = String::with_capacity(style.len());
    let mut last = 0usize;
    for caps in re.captures_iter(style) {
        let m = caps.get(0).expect("match");
        out.push_str(&style[last..m.start()]);
        let url_raw = caps
            .name("u_d")
            .or_else(|| caps.name("u_s"))
            .or_else(|| caps.name("u2"))
            .map(|m| m.as_str().trim().trim_matches('"').trim_matches('\''))
            .unwrap_or_default();
        if url_raw.starts_with("data:") || url_raw.starts_with('#') || url_raw.is_empty() {
            out.push_str(m.as_str());
            last = m.end();
            continue;
        }
        let url = resolve_any_url(base_url, url_raw)?;
        let req = AssetRequest {
            kind: AssetKind::Image,
            source: AssetSource::Remote(url),
        };
        let replacement = store.get(req).await?;
        out.push_str("url(\"");
        out.push_str(&replacement.replace('"', "\\\""));
        out.push_str("\")");
        last = m.end();
    }
    out.push_str(&style[last..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn srcset_choose_best() {
        assert_eq!(
            choose_best_src_from_srcset("a.png 1x, b.png 2x").as_deref(),
            Some("b.png")
        );
        assert_eq!(
            choose_best_src_from_srcset("a.png 100w, b.png 300w").as_deref(),
            Some("b.png")
        );
    }

    #[test]
    fn topic_anchor_rewrite() {
        let base = Url::parse("https://forum.example.com/").unwrap();
        assert_eq!(
            topic_local_anchor(&base, 123, "/t/slug/123/5").as_deref(),
            Some("#post_5")
        );
        assert_eq!(
            topic_local_anchor(&base, 123, "https://forum.example.com/t/slug/123/5").as_deref(),
            Some("#post_5")
        );
        assert!(topic_local_anchor(&base, 999, "/t/slug/123/5").is_none());
    }
}
