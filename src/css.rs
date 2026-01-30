use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use kuchiki::traits::TendrilSink as _;
use regex::Regex;
use url::Url;

use crate::assets::{AssetKind, AssetRequest, AssetSource, AssetStore};
use crate::progress::DownloadKind;

#[derive(Debug, Clone)]
pub enum CssOrigin {
    Local(PathBuf),
    Remote(Url),
}

pub async fn bundle_css(
    base_url: &Url,
    css_files: &[PathBuf],
    store: &AssetStore,
) -> anyhow::Result<String> {
    let origins: Vec<CssOrigin> = css_files.iter().cloned().map(CssOrigin::Local).collect();
    bundle_css_origins(base_url, &origins, store).await
}

pub async fn bundle_css_origins(
    base_url: &Url,
    origins: &[CssOrigin],
    store: &AssetStore,
) -> anyhow::Result<String> {
    let mut visited = HashSet::<String>::new();
    let mut bundled = String::new();

    for (idx, origin) in origins.iter().enumerate() {
        let css = load_css_recursive(base_url, origin.clone(), store, &mut visited)
            .await
            .with_context(|| format!("process css {}", origin_key(origin)))?;
        if idx != 0 {
            bundled.push('\n');
        }
        bundled.push_str(&css);
        bundled.push('\n');
    }

    Ok(bundled)
}

pub async fn discover_css_origins_from_base_url(
    base_url: &Url,
    store: &AssetStore,
) -> anyhow::Result<Vec<CssOrigin>> {
    let html = store
        .fetch_remote_text(base_url.clone(), DownloadKind::Html)
        .await
        .with_context(|| format!("download html {}", base_url))?;

    let doc = kuchiki::parse_html().one(html);

    let mut out = Vec::<CssOrigin>::new();
    let mut seen = HashSet::<String>::new();

    if let Ok(nodes) = doc.select("link[href]") {
        for node in nodes {
            let attrs = node.attributes.borrow();
            let rel = attrs.get("rel").unwrap_or("");
            if !is_css_link_rel(rel, attrs.get("as")) {
                continue;
            }

            let href = attrs.get("href").unwrap_or("").trim();
            if href.is_empty() || is_non_fetchable_url(href) {
                continue;
            }

            let url = resolve_html_href(base_url, href)
                .with_context(|| format!("resolve css href {}", href))?;
            let key = url.as_str().to_string();
            if seen.insert(key) {
                out.push(CssOrigin::Remote(url));
            }
        }
    }

    Ok(out)
}

#[async_recursion::async_recursion]
async fn load_css_recursive(
    base_url: &Url,
    origin: CssOrigin,
    store: &AssetStore,
    visited: &mut HashSet<String>,
) -> anyhow::Result<String> {
    let key = origin_key(&origin);
    if visited.contains(&key) {
        return Ok(String::new());
    }
    visited.insert(key);

    let css = match &origin {
        CssOrigin::Local(path) => {
            std::fs::read_to_string(path).with_context(|| format!("read css {}", path.display()))?
        }
        CssOrigin::Remote(url) => store
            .fetch_remote_text(url.clone(), DownloadKind::Css)
            .await
            .with_context(|| format!("download css {}", url))?,
    };

    inline_imports_and_rewrite_urls(base_url, &origin, store, visited, &css).await
}

fn origin_key(origin: &CssOrigin) -> String {
    match origin {
        CssOrigin::Local(path) => format!("file:{}", path.display()),
        CssOrigin::Remote(url) => url.as_str().to_string(),
    }
}

fn is_css_link_rel(rel: &str, as_attr: Option<&str>) -> bool {
    let rel_tokens = rel
        .split(|c: char| c.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if rel_tokens.iter().any(|t| t == "stylesheet") {
        return true;
    }

    // Some sites use <link rel="preload" as="style" href="...">.
    if rel_tokens.iter().any(|t| t == "preload") {
        if let Some(as_attr) = as_attr {
            if as_attr.eq_ignore_ascii_case("style") {
                return true;
            }
        }
    }

    false
}

fn resolve_html_href(base_url: &Url, href: &str) -> anyhow::Result<Url> {
    let h = href.trim();
    if h.starts_with("http://") || h.starts_with("https://") {
        return Ok(Url::parse(h)?);
    }
    if h.starts_with("//") {
        return Ok(Url::parse(&format!("{}:{}", base_url.scheme(), h))?);
    }
    Ok(base_url.join(h)?)
}

async fn inline_imports_and_rewrite_urls(
    base_url: &Url,
    origin: &CssOrigin,
    store: &AssetStore,
    visited: &mut HashSet<String>,
    css: &str,
) -> anyhow::Result<String> {
    let import_re = Regex::new(
        r#"@import\s+(?:url\(\s*)?(?:(?:"(?P<u_d>[^"]+)"|'(?P<u_s>[^']+)'|(?P<u2>[^);]+)))\s*\)?\s*(?P<media>[^;]*)\s*;"#,
    )
    .expect("import regex");

    let mut out = String::with_capacity(css.len());
    let mut last = 0usize;
    for caps in import_re.captures_iter(css) {
        let m = caps.get(0).expect("match");
        out.push_str(
            rewrite_css_urls(base_url, origin, store, &css[last..m.start()])
                .await?
                .as_str(),
        );

        let url_raw = caps
            .name("u_d")
            .or_else(|| caps.name("u_s"))
            .or_else(|| caps.name("u2"))
            .map(|m| m.as_str().trim())
            .unwrap_or_default();
        let media = caps.name("media").map(|m| m.as_str().trim()).unwrap_or("");

        let imported_origin = resolve_import_origin(base_url, origin, url_raw)
            .with_context(|| format!("resolve @import {}", url_raw))?;
        let imported_css = load_css_recursive(base_url, imported_origin, store, visited).await?;

        if media.is_empty() {
            out.push_str(&imported_css);
        } else {
            out.push_str("@media ");
            out.push_str(media);
            out.push_str(" {");
            out.push_str(&imported_css);
            out.push_str("}\n");
        }

        last = m.end();
    }

    out.push_str(
        rewrite_css_urls(base_url, origin, store, &css[last..])
            .await?
            .as_str(),
    );
    Ok(out)
}

async fn rewrite_css_urls(
    base_url: &Url,
    origin: &CssOrigin,
    store: &AssetStore,
    css: &str,
) -> anyhow::Result<String> {
    let url_re =
        Regex::new(r#"url\(\s*(?:(?:"(?P<u_d>[^"]+)"|'(?P<u_s>[^']+)'|(?P<u2>[^)]+)))\s*\)"#)
            .expect("url regex");

    let mut out = String::with_capacity(css.len());
    let mut last = 0usize;
    for caps in url_re.captures_iter(css) {
        let m = caps.get(0).expect("match");
        out.push_str(&css[last..m.start()]);

        let url_raw = caps
            .name("u_d")
            .or_else(|| caps.name("u_s"))
            .or_else(|| caps.name("u2"))
            .map(|m| m.as_str().trim().trim_matches('"').trim_matches('\''))
            .unwrap_or_default();

        if is_non_fetchable_url(url_raw) {
            out.push_str(m.as_str());
            last = m.end();
            continue;
        }

        let resolved = resolve_css_url(base_url, origin, url_raw)
            .with_context(|| format!("resolve css url {}", url_raw))?;
        let kind = guess_asset_kind(&resolved, url_raw);
        let req = match resolved {
            ResolvedAsset::Remote(url) => AssetRequest {
                kind,
                source: AssetSource::Remote(url),
            },
            ResolvedAsset::Local(path) => AssetRequest {
                kind,
                source: AssetSource::Local(path),
            },
        };

        let replacement = match store.get(req).await {
            Ok(v) => v,
            Err(e) => {
                if matches!(kind, AssetKind::Font) {
                    tracing::warn!(error = %e, url = %url_raw, "font download failed; falling back");
                    // Strict offline: no network. Provide an empty data URI for fonts so the CSS remains valid enough to fallback.
                    "data:font/woff2;base64,".to_string()
                } else {
                    return Err(e).with_context(|| format!("download asset {}", url_raw));
                }
            }
        };

        let replacement = if matches!(store.output_mode(), crate::assets::OutputMode::Dir) {
            relativize_for_bundled_css(&replacement, store.assets_dir_name())
        } else {
            replacement
        };

        out.push_str("url(\"");
        out.push_str(&escape_double_quotes(&replacement));
        out.push_str("\")");

        last = m.end();
    }

    out.push_str(&css[last..]);
    Ok(out)
}

#[derive(Debug)]
enum ResolvedAsset {
    Remote(Url),
    Local(PathBuf),
}

fn resolve_import_origin(
    base_url: &Url,
    origin: &CssOrigin,
    raw: &str,
) -> anyhow::Result<CssOrigin> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(CssOrigin::Remote(Url::parse(raw)?));
    }
    if raw.starts_with("//") {
        let u = Url::parse(&format!("{}:{}", base_url.scheme(), raw))?;
        return Ok(CssOrigin::Remote(u));
    }
    if raw.starts_with('/') {
        return match origin {
            CssOrigin::Remote(url) => Ok(CssOrigin::Remote(url.join(raw)?)),
            CssOrigin::Local(_) => Ok(CssOrigin::Remote(base_url.join(raw)?)),
        };
    }

    match origin {
        CssOrigin::Local(path) => {
            let base = path.parent().unwrap_or(Path::new("."));
            Ok(CssOrigin::Local(base.join(raw)))
        }
        CssOrigin::Remote(url) => Ok(CssOrigin::Remote(url.join(raw)?)),
    }
}

fn resolve_css_url(base_url: &Url, origin: &CssOrigin, raw: &str) -> anyhow::Result<ResolvedAsset> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(ResolvedAsset::Remote(Url::parse(raw)?));
    }
    if raw.starts_with("//") {
        let u = Url::parse(&format!("{}:{}", base_url.scheme(), raw))?;
        return Ok(ResolvedAsset::Remote(u));
    }
    if raw.starts_with('/') {
        return match origin {
            CssOrigin::Remote(url) => Ok(ResolvedAsset::Remote(url.join(raw)?)),
            CssOrigin::Local(_) => Ok(ResolvedAsset::Remote(base_url.join(raw)?)),
        };
    }

    match origin {
        CssOrigin::Local(path) => {
            let base = path.parent().unwrap_or(Path::new("."));
            Ok(ResolvedAsset::Local(base.join(raw)))
        }
        CssOrigin::Remote(url) => Ok(ResolvedAsset::Remote(url.join(raw)?)),
    }
}

fn is_non_fetchable_url(url: &str) -> bool {
    let u = url.trim();
    u.is_empty()
        || u.starts_with("data:")
        || u.starts_with("about:")
        || u.starts_with('#')
        || u.starts_with("blob:")
}

fn escape_double_quotes(s: &str) -> String {
    s.replace('"', "\\\"")
}

fn relativize_for_bundled_css(replacement: &str, assets_dir_name: &str) -> String {
    if replacement.starts_with("data:") {
        return replacement.to_string();
    }

    let prefix = format!("{}/", assets_dir_name);
    if let Some(stripped) = replacement.strip_prefix(&prefix) {
        return format!("../{}", stripped);
    }

    replacement.to_string()
}

fn guess_asset_kind(resolved: &ResolvedAsset, raw: &str) -> AssetKind {
    let ext = match resolved {
        ResolvedAsset::Remote(url) => url
            .path()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase(),
        ResolvedAsset::Local(path) => path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase(),
    };
    if matches!(ext.as_str(), "woff2" | "woff" | "ttf" | "otf" | "eot") {
        return AssetKind::Font;
    }
    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif"
    ) {
        return AssetKind::Image;
    }
    if raw.contains("fonts.googleapis.com") || raw.contains("fonts.gstatic.com") {
        return AssetKind::Font;
    }
    AssetKind::Other
}
