use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use url::Url;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Mode {
    Dir,
    Single,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OfflineMode {
    Strict,
    #[value(hide = true)]
    Hybrid,
    #[value(hide = true)]
    Loose,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProgressMode {
    /// Enable progress UI when stderr is a TTY.
    Auto,
    /// Always enable progress UI (even when piped).
    Always,
    /// Never show progress UI.
    Never,
}

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Args {
    /// Discourse topic JSON file (must include all posts with `cooked` HTML).
    #[arg(long)]
    pub input: PathBuf,

    /// Base URL of the Discourse site, used to resolve relative URLs (e.g. `https://forum.example.com`).
    #[arg(long)]
    pub base_url: Url,

    /// One or more local CSS files exported from the site.
    ///
    /// If omitted, the tool will try to fetch the site's HTML from `--base-url` and discover `<link rel="stylesheet" ...>`
    /// CSS URLs automatically.
    #[arg(long)]
    pub css: Vec<PathBuf>,

    /// Use the built-in minimal theme CSS (light/dark) and skip crawling site CSS.
    ///
    /// When enabled, the tool will NOT auto-discover stylesheets from `--base-url`, and will ignore `--css`.
    #[arg(long)]
    pub builtin_css: bool,

    /// Output mode: `dir` (HTML + assets/) or `single` (one self-contained HTML).
    #[arg(long, value_enum, default_value = "dir")]
    pub mode: Mode,

    /// Offline mode (v1 only supports `strict`).
    #[arg(long, value_enum, default_value = "strict")]
    pub offline: OfflineMode,

    /// Output path. For `dir` mode: a directory. For `single` mode: an HTML file path.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Avatar size for `{size}` substitution in `avatar_template`.
    #[arg(long, default_value_t = 120)]
    pub avatar_size: u32,

    /// Assets directory name for `dir` mode.
    #[arg(long, default_value = "assets")]
    pub assets_dir_name: String,

    /// Max concurrent downloads.
    #[arg(long, default_value_t = 8)]
    pub max_concurrency: usize,

    /// HTTP User-Agent used for downloading assets.
    #[arg(long, default_value = "discourse-topic-render/0.1")]
    pub user_agent: String,

    /// Progress display: `auto`, `always`, or `never`.
    #[arg(long, value_enum, default_value = "auto")]
    pub progress: ProgressMode,
}
