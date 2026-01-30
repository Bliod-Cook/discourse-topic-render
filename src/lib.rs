mod assets;
mod builtin;
mod cli;
mod css;
mod fetcher;
mod html;
mod progress;
mod strict;
mod topic;

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use assets::AssetStore;
use cli::Args;
use fetcher::Fetcher;

pub use cli::ProgressMode;
pub use cli::{Args as CliArgs, Mode, OfflineMode};

pub async fn run(args: Args) -> anyhow::Result<()> {
    use std::io::IsTerminal as _;

    if !matches!(args.offline, OfflineMode::Strict) {
        anyhow::bail!("only --offline strict is supported in v1");
    }

    let progress_enabled = match args.progress {
        ProgressMode::Always => true,
        ProgressMode::Never => false,
        ProgressMode::Auto => std::io::stderr().is_terminal(),
    };
    let progress = progress::Progress::new(progress_enabled, args.max_concurrency);
    progress.set_stage("读取 topic.json");

    let topic: topic::TopicJson = {
        let bytes =
            std::fs::read(&args.input).with_context(|| format!("read {}", args.input.display()))?;
        serde_json::from_slice(&bytes).context("parse topic.json")?
    };

    let total_posts = topic
        .post_stream
        .posts
        .iter()
        .filter(|p| p.cooked.as_deref().unwrap_or("").trim().len() > 0)
        .count();
    progress.set_posts_total(total_posts);

    let fetcher = Fetcher::new(
        &args.user_agent,
        args.max_concurrency,
        Some(progress.clone()),
    )?;

    let res = match args.mode {
        Mode::Dir => render_dir(&topic, &args, fetcher, progress.clone()).await,
        Mode::Single => render_single(&topic, &args, fetcher, progress.clone()).await,
    };
    progress.finish();
    res
}

async fn render_dir(
    topic: &topic::TopicJson,
    args: &Args,
    fetcher: Fetcher,
    progress: std::sync::Arc<progress::Progress>,
) -> anyhow::Result<()> {
    let out_dir = args.out.clone().unwrap_or_else(|| PathBuf::from("out"));
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let store = AssetStore::new_dir(
        out_dir.clone(),
        args.assets_dir_name.clone(),
        fetcher.clone(),
        Some(progress.clone()),
    );

    progress.set_stage("打包 CSS");
    let css_text = bundle_css_for_args(args, &store).await?;
    let css_rel = write_css_file(&out_dir, &args.assets_dir_name, &css_text)?;

    progress.set_stage("渲染帖子");
    let posts = html::render_posts(topic, &args.base_url, args.avatar_size, &store).await?;

    progress.set_stage("生成 HTML");
    let html = if args.builtin_css {
        html::build_html_minimal(topic, &posts, "", Some(&css_rel))
    } else {
        html::build_html(topic, &posts, "", Some(&css_rel))
    };
    strict::assert_strict_offline(&html, &css_text)?;

    progress.set_stage("写入输出");
    let html_path = out_dir.join(format!("topic-{}.html", topic.id));
    std::fs::write(&html_path, html).with_context(|| format!("write {}", html_path.display()))?;

    Ok(())
}

async fn render_single(
    topic: &topic::TopicJson,
    args: &Args,
    fetcher: Fetcher,
    progress: std::sync::Arc<progress::Progress>,
) -> anyhow::Result<()> {
    let out_path = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("topic-{}.html", topic.id)));

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }

    let out_dir = out_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let store = AssetStore::new_single(out_dir, fetcher.clone(), Some(progress.clone()));

    progress.set_stage("打包 CSS");
    let css_text = bundle_css_for_args(args, &store).await?;
    progress.set_stage("渲染帖子");
    let posts = html::render_posts(topic, &args.base_url, args.avatar_size, &store).await?;

    progress.set_stage("生成 HTML");
    let html = if args.builtin_css {
        html::build_html_minimal(topic, &posts, &css_text, None)
    } else {
        html::build_html(topic, &posts, &css_text, None)
    };
    strict::assert_strict_offline(&html, &css_text)?;

    progress.set_stage("写入输出");
    std::fs::write(&out_path, html).with_context(|| format!("write {}", out_path.display()))?;
    Ok(())
}

async fn bundle_css_for_args(args: &Args, store: &AssetStore) -> anyhow::Result<String> {
    if args.builtin_css {
        if !args.css.is_empty() {
            tracing::warn!("--builtin-css is set; ignoring --css");
        }
        return Ok(builtin::BUILTIN_CSS.to_string());
    }

    if !args.css.is_empty() {
        return css::bundle_css(&args.base_url, &args.css, store).await;
    }

    let origins = css::discover_css_origins_from_base_url(&args.base_url, store).await?;
    if origins.is_empty() {
        anyhow::bail!(
            "no CSS discovered from {}; pass one or more --css <file> paths",
            args.base_url
        );
    }

    tracing::info!(count = origins.len(), "auto-discovered css stylesheets");
    css::bundle_css_origins(&args.base_url, &origins, store).await
}

fn write_css_file(out_dir: &Path, assets_dir_name: &str, css: &str) -> anyhow::Result<String> {
    let rel = format!("{}/css/site.css", assets_dir_name);
    let abs = out_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&abs, css).with_context(|| format!("write {}", abs.display()))?;
    Ok(rel)
}
