use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use indicatif::{
    HumanBytes, HumanDuration, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle,
};
use url::Url;

use crate::assets::AssetKind;

#[derive(Debug, Clone, Copy)]
pub enum DownloadKind {
    Html,
    Css,
    Asset(AssetKind),
}

impl DownloadKind {
    fn label(self) -> &'static str {
        match self {
            DownloadKind::Html => "html",
            DownloadKind::Css => "css",
            DownloadKind::Asset(AssetKind::Avatar) => "avatar",
            DownloadKind::Asset(AssetKind::Image) => "image",
            DownloadKind::Asset(AssetKind::Font) => "font",
            DownloadKind::Asset(AssetKind::Other) => "other",
        }
    }
}

#[derive(Debug, Default)]
struct DownloadCounters {
    html: AtomicU64,
    css: AtomicU64,
    avatar: AtomicU64,
    image: AtomicU64,
    font: AtomicU64,
    other: AtomicU64,
}

impl DownloadCounters {
    fn inc(&self, kind: DownloadKind) {
        match kind {
            DownloadKind::Html => {
                self.html.fetch_add(1, Ordering::Relaxed);
            }
            DownloadKind::Css => {
                self.css.fetch_add(1, Ordering::Relaxed);
            }
            DownloadKind::Asset(AssetKind::Avatar) => {
                self.avatar.fetch_add(1, Ordering::Relaxed);
            }
            DownloadKind::Asset(AssetKind::Image) => {
                self.image.fetch_add(1, Ordering::Relaxed);
            }
            DownloadKind::Asset(AssetKind::Font) => {
                self.font.fetch_add(1, Ordering::Relaxed);
            }
            DownloadKind::Asset(AssetKind::Other) => {
                self.other.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn snapshot(&self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.html.load(Ordering::Relaxed),
            self.css.load(Ordering::Relaxed),
            self.avatar.load(Ordering::Relaxed),
            self.image.load(Ordering::Relaxed),
            self.font.load(Ordering::Relaxed),
            self.other.load(Ordering::Relaxed),
        )
    }
}

pub struct Progress {
    enabled: bool,
    start: Instant,
    max_concurrency: usize,

    // UI
    mp: Option<MultiProgress>,
    stage: ProgressBar,
    posts: ProgressBar,
    downloads: ProgressBar,

    // Counters
    posts_total: AtomicU64,
    posts_done: AtomicU64,

    asset_requests_total: AtomicU64,
    asset_requests_unique: AtomicU64,
    asset_requests_cache_hit: AtomicU64,

    http_in_flight: AtomicU64,
    http_done: AtomicU64,
    http_bytes: AtomicU64,

    done_by_kind: DownloadCounters,
    last_http_label: Mutex<String>,
}

impl Progress {
    pub fn new(enabled: bool, max_concurrency: usize) -> Arc<Self> {
        let start = Instant::now();

        if !enabled {
            return Arc::new(Self {
                enabled: false,
                start,
                max_concurrency: max_concurrency.max(1),
                mp: None,
                stage: ProgressBar::hidden(),
                posts: ProgressBar::hidden(),
                downloads: ProgressBar::hidden(),
                posts_total: AtomicU64::new(0),
                posts_done: AtomicU64::new(0),
                asset_requests_total: AtomicU64::new(0),
                asset_requests_unique: AtomicU64::new(0),
                asset_requests_cache_hit: AtomicU64::new(0),
                http_in_flight: AtomicU64::new(0),
                http_done: AtomicU64::new(0),
                http_bytes: AtomicU64::new(0),
                done_by_kind: DownloadCounters::default(),
                last_http_label: Mutex::new(String::new()),
            });
        }

        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());

        let stage = mp.add(ProgressBar::new_spinner());
        stage.set_style(
            ProgressStyle::with_template("{spinner} {msg}  [{elapsed_precise}]").unwrap(),
        );
        stage.enable_steady_tick(Duration::from_millis(80));
        stage.set_message("准备开始");

        let posts = mp.add(ProgressBar::new(0));
        posts.set_style(
            ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        posts.set_message("posts");

        let downloads = mp.add(ProgressBar::new_spinner());
        downloads.set_style(
            ProgressStyle::with_template("{spinner} {msg}  [{elapsed_precise}]").unwrap(),
        );
        downloads.enable_steady_tick(Duration::from_millis(120));
        downloads.set_message("下载统计");

        Arc::new(Self {
            enabled: true,
            start,
            max_concurrency: max_concurrency.max(1),
            mp: Some(mp),
            stage,
            posts,
            downloads,
            posts_total: AtomicU64::new(0),
            posts_done: AtomicU64::new(0),
            asset_requests_total: AtomicU64::new(0),
            asset_requests_unique: AtomicU64::new(0),
            asset_requests_cache_hit: AtomicU64::new(0),
            http_in_flight: AtomicU64::new(0),
            http_done: AtomicU64::new(0),
            http_bytes: AtomicU64::new(0),
            done_by_kind: DownloadCounters::default(),
            last_http_label: Mutex::new(String::new()),
        })
    }

    pub fn set_stage(&self, msg: impl Into<String>) {
        if !self.enabled {
            return;
        }
        self.stage.set_message(msg.into());
    }

    pub fn set_posts_total(&self, total: usize) {
        self.posts_total.store(total as u64, Ordering::Relaxed);
        if self.enabled {
            self.posts.set_length(total as u64);
        }
    }

    pub fn post_done(&self, post_number: u64) {
        self.posts_done.fetch_add(1, Ordering::Relaxed);
        if self.enabled {
            self.posts.inc(1);
            self.posts.set_message(format!("post #{post_number}"));
        }
    }

    pub fn asset_request(&self, _kind: AssetKind, is_unique: bool) {
        self.asset_requests_total.fetch_add(1, Ordering::Relaxed);
        if is_unique {
            self.asset_requests_unique.fetch_add(1, Ordering::Relaxed);
        } else {
            self.asset_requests_cache_hit
                .fetch_add(1, Ordering::Relaxed);
        }

        if self.enabled && (self.asset_requests_total.load(Ordering::Relaxed) % 8) == 0 {
            // Keep the UI reasonably fresh without over-allocating.
            self.refresh_downloads();
        }
    }

    pub fn http_start(&self, kind: DownloadKind, url: &Url) {
        self.http_in_flight.fetch_add(1, Ordering::Relaxed);
        if self.enabled {
            if let Ok(mut last) = self.last_http_label.lock() {
                *last = format!("GET {} ({})", url, kind.label());
            }
            self.set_stage(format!("下载 {} ...", kind.label()));
            self.refresh_downloads();
        }
    }

    pub fn http_throttled(&self, kind: DownloadKind, url: &Url, status: u16, wait: Duration) {
        if !self.enabled {
            return;
        }
        if let Ok(mut last) = self.last_http_label.lock() {
            *last = format!(
                "GET {} ({}) throttled {} wait {}ms",
                url,
                kind.label(),
                status,
                wait.as_millis()
            );
        }
        self.refresh_downloads();
    }

    pub fn http_ok(&self, kind: DownloadKind, url: &Url, bytes: usize) {
        self.http_in_flight.fetch_sub(1, Ordering::Relaxed);
        self.http_done.fetch_add(1, Ordering::Relaxed);
        self.http_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.done_by_kind.inc(kind);

        if self.enabled {
            if let Ok(mut last) = self.last_http_label.lock() {
                *last = format!("GET {} ({}) ok {}B", url, kind.label(), bytes);
            }
            self.refresh_downloads();
        }
    }

    pub fn http_err(&self, kind: DownloadKind, url: &Url) {
        self.http_in_flight.fetch_sub(1, Ordering::Relaxed);
        if self.enabled {
            if let Ok(mut last) = self.last_http_label.lock() {
                *last = format!("GET {} ({}) failed", url, kind.label());
            }
            self.refresh_downloads();
        }
    }

    pub fn finish(&self) {
        if !self.enabled {
            return;
        }
        self.refresh_downloads();
        self.stage.finish_with_message("完成");
        self.posts.finish_and_clear();
        self.downloads.finish_and_clear();
        if let Some(mp) = &self.mp {
            // Best effort: ensure the last render flushes.
            let _ = mp.println(format!("Done in {}", HumanDuration(self.start.elapsed())));
        }
    }

    fn refresh_downloads(&self) {
        if !self.enabled {
            return;
        }

        let in_flight = self.http_in_flight.load(Ordering::Relaxed);
        let done = self.http_done.load(Ordering::Relaxed);
        let bytes = self.http_bytes.load(Ordering::Relaxed);
        let asset_total = self.asset_requests_total.load(Ordering::Relaxed);
        let asset_unique = self.asset_requests_unique.load(Ordering::Relaxed);
        let asset_hit = self.asset_requests_cache_hit.load(Ordering::Relaxed);
        let posts_done = self.posts_done.load(Ordering::Relaxed);
        let posts_total = self.posts_total.load(Ordering::Relaxed);
        let (html, css, avatar, image, font, other) = self.done_by_kind.snapshot();

        let elapsed = self.start.elapsed().as_secs_f64().max(0.001);
        let rate = (bytes as f64 / elapsed) as u64;

        let last = self
            .last_http_label
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        self.downloads.set_message(format!(
            "HTTP: done {done} | in-flight {in_flight}/{max} | bytes {bytes} ({rate}/s) | assets req {asset_total} uniq {asset_unique} hit {asset_hit} | posts {posts_done}/{posts_total} | html {html} css {css} avatar {avatar} img {image} font {font} other {other} | {last}",
            max = self.max_concurrency,
            bytes = HumanBytes(bytes),
            rate = HumanBytes(rate),
        ));
    }
}
