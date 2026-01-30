use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use base64::Engine as _;
use url::Url;

use crate::fetcher::Fetcher;

#[derive(Debug, Clone, Copy)]
pub enum AssetKind {
    Avatar,
    Image,
    Font,
    Other,
}

#[derive(Debug, Clone)]
pub enum AssetSource {
    Remote(Url),
    Local(PathBuf),
}

#[derive(Debug, Clone)]
pub struct AssetRequest {
    pub kind: AssetKind,
    pub source: AssetSource,
}

#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    Dir,
    Single,
}

pub struct AssetStore {
    mode: OutputMode,
    out_dir: PathBuf,
    assets_dir_name: String,
    fetcher: Fetcher,
    entries: tokio::sync::Mutex<
        HashMap<String, std::sync::Arc<tokio::sync::OnceCell<Result<String, String>>>>,
    >,
}

impl AssetStore {
    pub fn new_dir(out_dir: PathBuf, assets_dir_name: String, fetcher: Fetcher) -> Self {
        Self {
            mode: OutputMode::Dir,
            out_dir,
            assets_dir_name,
            fetcher,
            entries: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn new_single(out_dir: PathBuf, fetcher: Fetcher) -> Self {
        Self {
            mode: OutputMode::Single,
            out_dir,
            assets_dir_name: "assets".to_string(),
            fetcher,
            entries: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn get(&self, request: AssetRequest) -> anyhow::Result<String> {
        let key = request_key(&request);
        let cell = {
            let mut entries = self.entries.lock().await;
            entries
                .entry(key)
                .or_insert_with(|| std::sync::Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        let stored = cell
            .get_or_init(|| async {
                match self.fetch_and_store(&request).await {
                    Ok(v) => Ok(v),
                    Err(e) => Err(format!("{:#}", e)),
                }
            })
            .await;

        match stored {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        }
    }

    pub async fn fetch_remote_text(&self, url: Url) -> anyhow::Result<String> {
        let (bytes, _headers) = self.fetcher.get_bytes(url.clone()).await?;
        let text = String::from_utf8(bytes.to_vec())
            .with_context(|| format!("remote text at {} is not valid utf-8", url))?;
        Ok(text)
    }

    pub fn output_mode(&self) -> OutputMode {
        self.mode
    }

    pub fn assets_dir_name(&self) -> &str {
        &self.assets_dir_name
    }

    async fn fetch_and_store(&self, request: &AssetRequest) -> anyhow::Result<String> {
        let (bytes, content_type_hint) = match &request.source {
            AssetSource::Remote(url) => {
                let (bytes, headers) = self.fetcher.get_bytes(url.clone()).await?;
                let ct = headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                (bytes.to_vec(), ct)
            }
            AssetSource::Local(path) => {
                let bytes = std::fs::read(path)
                    .with_context(|| format!("read local asset {}", path.display()))?;
                (bytes, None)
            }
        };

        let (mime, ext) = sniff_mime_and_ext(&bytes, content_type_hint.as_deref(), request);

        match self.mode {
            OutputMode::Single => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Ok(format!("data:{};base64,{}", mime, b64))
            }
            OutputMode::Dir => {
                let rel_path = write_asset_file(
                    &self.out_dir,
                    &self.assets_dir_name,
                    request.kind,
                    &bytes,
                    &ext,
                )?;
                Ok(rel_path)
            }
        }
    }
}

fn request_key(request: &AssetRequest) -> String {
    match &request.source {
        AssetSource::Remote(url) => url.as_str().to_string(),
        AssetSource::Local(path) => format!("file:{}", path.display()),
    }
}

fn kind_subdir(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Avatar => "avatar",
        AssetKind::Image => "img",
        AssetKind::Font => "font",
        AssetKind::Other => "other",
    }
}

fn write_asset_file(
    out_dir: &Path,
    assets_dir_name: &str,
    kind: AssetKind,
    bytes: &[u8],
    ext: &str,
) -> anyhow::Result<String> {
    let hash = blake3::hash(bytes).to_hex().to_string();
    let rel = format!("{}/{}/{}.{}", assets_dir_name, kind_subdir(kind), hash, ext);
    let abs = out_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if !abs.exists() {
        std::fs::write(&abs, bytes).with_context(|| format!("write {}", abs.display()))?;
    }
    Ok(rel)
}

fn sniff_mime_and_ext(
    bytes: &[u8],
    content_type_hint: Option<&str>,
    request: &AssetRequest,
) -> (String, String) {
    if let Some(ct) = content_type_hint.and_then(|s| s.split(';').next()) {
        if let Some((mime, ext)) = mime_to_ext(ct.trim(), request) {
            return (mime.to_string(), ext.to_string());
        }
    }

    // Best-effort magic bytes
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return ("image/png".to_string(), "png".to_string());
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return ("image/jpeg".to_string(), "jpg".to_string());
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return ("image/gif".to_string(), "gif".to_string());
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return ("image/webp".to_string(), "webp".to_string());
    }
    if bytes.starts_with(b"wOFF") {
        return ("font/woff".to_string(), "woff".to_string());
    }
    if bytes.starts_with(b"wOF2") {
        return ("font/woff2".to_string(), "woff2".to_string());
    }
    if bytes.starts_with(b"OTTO") {
        return ("font/otf".to_string(), "otf".to_string());
    }
    if bytes.starts_with(b"\x00\x01\x00\x00") {
        return ("font/ttf".to_string(), "ttf".to_string());
    }

    // Fall back to URL extension for remote assets.
    if let AssetSource::Remote(url) = &request.source {
        if let Some((mime, ext)) = ext_from_url(url, request) {
            return (mime, ext);
        }
    }

    // Default.
    ("application/octet-stream".to_string(), "bin".to_string())
}

fn mime_to_ext(mime: &str, request: &AssetRequest) -> Option<(&'static str, &'static str)> {
    match mime {
        "image/png" => Some(("image/png", "png")),
        "image/jpeg" => Some(("image/jpeg", "jpg")),
        "image/gif" => Some(("image/gif", "gif")),
        "image/webp" => Some(("image/webp", "webp")),
        "image/svg+xml" => Some(("image/svg+xml", "svg")),
        "font/woff2" => Some(("font/woff2", "woff2")),
        "font/woff" => Some(("font/woff", "woff")),
        "application/font-woff2" => Some(("font/woff2", "woff2")),
        "application/font-woff" => Some(("font/woff", "woff")),
        "application/octet-stream" => match request.kind {
            AssetKind::Font => Some(("font/woff2", "woff2")),
            _ => None,
        },
        _ => None,
    }
}

fn ext_from_url(url: &Url, request: &AssetRequest) -> Option<(String, String)> {
    let path = url.path();
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let (mime, ext) = match ext.as_str() {
        "png" => ("image/png", "png"),
        "jpg" | "jpeg" => ("image/jpeg", "jpg"),
        "gif" => ("image/gif", "gif"),
        "webp" => ("image/webp", "webp"),
        "svg" => ("image/svg+xml", "svg"),
        "woff2" => ("font/woff2", "woff2"),
        "woff" => ("font/woff", "woff"),
        "ttf" => ("font/ttf", "ttf"),
        "otf" => ("font/otf", "otf"),
        "eot" => ("application/vnd.ms-fontobject", "eot"),
        _ => match request.kind {
            AssetKind::Font => ("font/woff2", "woff2"),
            _ => return None,
        },
    };
    Some((mime.to_string(), ext.to_string()))
}
