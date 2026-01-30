use kuchiki::traits::TendrilSink as _;

pub fn assert_strict_offline(html: &str, css: &str) -> anyhow::Result<()> {
    assert_css_strict(css)?;
    assert_html_strict(html)?;
    Ok(())
}

fn assert_css_strict(css: &str) -> anyhow::Result<()> {
    let lowered = css.to_ascii_lowercase();
    if lowered.contains("url(http://")
        || lowered.contains("url(https://")
        || lowered.contains("url(\"http://")
        || lowered.contains("url(\"https://")
        || lowered.contains("url('//")
        || lowered.contains("url(\"//")
        || lowered.contains("url(/")
        || lowered.contains("url(\"/")
        || lowered.contains("url('/")
        || lowered.contains("@import \"http")
        || lowered.contains("@import url(http")
        || lowered.contains("@import url(\"http")
    {
        anyhow::bail!("strict offline check failed: css still references non-local urls");
    }
    Ok(())
}

fn assert_html_strict(html: &str) -> anyhow::Result<()> {
    let doc = kuchiki::parse_html().one(html);

    for selector in [
        "img[src]",
        "img[srcset]",
        "source[src]",
        "source[srcset]",
        "script[src]",
        "link[href]",
        "iframe[src]",
        "audio[src]",
        "video[src]",
    ] {
        if let Ok(nodes) = doc.select(selector) {
            for node in nodes {
                let attrs = node.attributes.borrow();
                for attr in ["src", "srcset", "href"] {
                    if let Some(v) = attrs.get(attr) {
                        if is_disallowed_autoload(v) {
                            anyhow::bail!(
                                "strict offline check failed: <{} {}=\"{}\"> is not local",
                                node.name.local.as_ref(),
                                attr,
                                v
                            );
                        }
                    }
                }
            }
        }
    }

    // Inline styles (attrs + <style>) should not have remote `url(http...)`.
    if let Ok(nodes) = doc.select("[style]") {
        for node in nodes {
            if let Some(style) = node.attributes.borrow().get("style") {
                if style.to_ascii_lowercase().contains("url(http") || style.contains("url(//") {
                    anyhow::bail!(
                        "strict offline check failed: style attribute contains remote url()"
                    );
                }
            }
        }
    }
    if let Ok(nodes) = doc.select("style") {
        for node in nodes {
            let text = node.text_contents();
            let lowered = text.to_ascii_lowercase();
            if lowered.contains("url(http")
                || lowered.contains("url(//")
                || lowered.contains("@import")
            {
                anyhow::bail!("strict offline check failed: <style> contains remote url()");
            }
        }
    }

    Ok(())
}

fn is_remote_auto_load(v: &str) -> bool {
    let s = v.trim().to_ascii_lowercase();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("//")
}

fn is_disallowed_autoload(v: &str) -> bool {
    let s = v.trim();
    if s.is_empty() {
        return false;
    }
    let lowered = s.to_ascii_lowercase();
    if lowered.starts_with("data:")
        || lowered.starts_with("about:")
        || lowered.starts_with("blob:")
        || lowered.starts_with('#')
    {
        return false;
    }
    is_remote_auto_load(s) || s.starts_with('/')
}
