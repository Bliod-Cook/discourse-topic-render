use std::path::Path;

use httpmock::Method::GET;
use httpmock::MockServer;
use tempfile::tempdir;
use url::Url;

fn png_bytes() -> Vec<u8> {
    // PNG signature + minimal IHDR chunk-ish bytes (not a valid image, but enough for sniffing).
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R',
    ]
}

fn woff2_bytes() -> Vec<u8> {
    // wOF2 signature + padding.
    vec![b'w', b'O', b'F', b'2', 0, 0, 0, 0]
}

fn read_to_string(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn assert_no_remote_autoload(html: &str) {
    for pat in [
        "img src=\"http://",
        "img src=\"https://",
        "img src=\"//",
        "script src=\"http",
        "link href=\"http",
        "<iframe",
    ] {
        assert!(
            !html.contains(pat),
            "unexpected remote autoload pattern {pat} in html"
        );
    }
}

#[tokio::test]
async fn renders_dir_and_single_offline() {
    let server = MockServer::start();

    for path in [
        "/avatar/120.png",
        "/img.png",
        "/img2.png",
        "/thumb.png",
        "/lightbox.png",
        "/bg.png",
    ] {
        server.mock(|when, then| {
            when.method(GET).path(path);
            then.status(200)
                .header("Content-Type", "image/png")
                .body(png_bytes());
        });
    }

    server.mock(|when, then| {
        when.method(GET).path("/font.woff2");
        then.status(200)
            .header("Content-Type", "font/woff2")
            .body(woff2_bytes());
    });

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("topic.json");
    let css = tmp.path().join("site.css");
    let imported_css = tmp.path().join("imported.css");
    let local_png = tmp.path().join("local.png");
    std::fs::write(&local_png, png_bytes()).unwrap();

    std::fs::write(
        &imported_css,
        r#"
@font-face {
  font-family: "Test";
  src: url("/font.woff2") format("woff2");
}
"#,
    )
    .unwrap();
    std::fs::write(
        &css,
        format!(
            r#"
@import "imported.css";
body {{
  background-image: url("/bg.png");
}}
.x {{
  background-image: url("./local.png");
}}
"#
        ),
    )
    .unwrap();

    let base_url = Url::parse(&server.url("/")).unwrap();
    let topic_json = format!(
        r#"{{
  "id": 123,
  "title": "Test Topic",
  "post_stream": {{
    "posts": [
      {{
        "id": 1,
        "post_number": 1,
        "username": "alice",
        "display_username": "alice",
        "avatar_template": "/avatar/{{size}}.png",
        "created_at": "2026-01-30T00:00:00.000Z",
        "cooked": "<p>Hello</p><p><img src=\"/img.png\" srcset=\"/img.png 1x, /img2.png 2x\"></p><p><a class=\"lightbox\" href=\"/lightbox.png\"><img src=\"/thumb.png\"></a></p><p><iframe src=\"https://example.com/embed\"></iframe></p><p><a href=\"/t/slug/123/1\">jump</a></p>"
      }}
    ]
  }}
}}"#
    );
    std::fs::write(&input, topic_json).unwrap();

    // dir mode
    let out_dir = tmp.path().join("out");
    let args = discourse_topic_render::CliArgs {
        input: input.clone(),
        base_url: base_url.clone(),
        css: vec![css.clone()],
        builtin_css: false,
        mode: discourse_topic_render::Mode::Dir,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_dir.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html_path = out_dir.join("topic-123.html");
    let css_path = out_dir.join("assets/css/site.css");
    assert!(html_path.exists());
    assert!(css_path.exists());

    let html = read_to_string(&html_path);
    let css_out = read_to_string(&css_path);
    assert_no_remote_autoload(&html);
    assert!(css_out.contains("url(\"../img/"));
    assert!(css_out.contains("url(\"../font/"));

    // single mode
    let out_single = tmp.path().join("topic-123-single.html");
    let args = discourse_topic_render::CliArgs {
        input,
        base_url,
        css: vec![css],
        builtin_css: false,
        mode: discourse_topic_render::Mode::Single,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_single.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html = read_to_string(&out_single);
    assert_no_remote_autoload(&html);
    assert!(html.contains("data:image/png;base64,"));
}

#[tokio::test]
async fn auto_discovers_css_when_not_provided() {
    let server = MockServer::start();

    // Homepage with stylesheet links.
    server.mock(|when, then| {
        when.method(GET).path("/");
        then.status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(
                r#"<!doctype html>
<html>
  <head>
    <link rel="stylesheet" href="/site.css">
  </head>
  <body>ok</body>
</html>"#,
            );
    });

    // CSS + imported CSS.
    server.mock(|when, then| {
        when.method(GET).path("/site.css");
        then.status(200).header("Content-Type", "text/css").body(
            r#"
@import "/imported.css";
body { background-image: url("/bg.png"); }
"#,
        );
    });
    server.mock(|when, then| {
        when.method(GET).path("/imported.css");
        then.status(200).header("Content-Type", "text/css").body(
            r#"
@font-face {
  font-family: "Test";
  src: url("/font.woff2") format("woff2");
}
"#,
        );
    });

    for path in ["/avatar/120.png", "/img.png", "/bg.png"] {
        server.mock(|when, then| {
            when.method(GET).path(path);
            then.status(200)
                .header("Content-Type", "image/png")
                .body(png_bytes());
        });
    }

    server.mock(|when, then| {
        when.method(GET).path("/font.woff2");
        then.status(200)
            .header("Content-Type", "font/woff2")
            .body(woff2_bytes());
    });

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("topic.json");

    let base_url = Url::parse(&server.url("/")).unwrap();
    let topic_json = r#"{
  "id": 123,
  "title": "Test Topic",
  "post_stream": {
    "posts": [
      {
        "id": 1,
        "post_number": 1,
        "username": "alice",
        "display_username": "alice",
        "avatar_template": "/avatar/{size}.png",
        "created_at": "2026-01-30T00:00:00.000Z",
        "cooked": "<p>Hello</p><p><img src=\"/img.png\"></p>"
      }
    ]
  }
}"#;
    std::fs::write(&input, topic_json).unwrap();

    // dir mode (no --css)
    let out_dir = tmp.path().join("out");
    let args = discourse_topic_render::CliArgs {
        input: input.clone(),
        base_url: base_url.clone(),
        css: vec![],
        builtin_css: false,
        mode: discourse_topic_render::Mode::Dir,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_dir.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html_path = out_dir.join("topic-123.html");
    let css_path = out_dir.join("assets/css/site.css");
    assert!(html_path.exists());
    assert!(css_path.exists());

    let html = read_to_string(&html_path);
    let css_out = read_to_string(&css_path);
    assert_no_remote_autoload(&html);
    assert!(css_out.contains("url(\"../img/"));
    assert!(css_out.contains("url(\"../font/"));

    // single mode (no --css)
    let out_single = tmp.path().join("topic-123-single.html");
    let args = discourse_topic_render::CliArgs {
        input,
        base_url,
        css: vec![],
        builtin_css: false,
        mode: discourse_topic_render::Mode::Single,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_single.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html = read_to_string(&out_single);
    assert_no_remote_autoload(&html);
    assert!(html.contains("data:image/png;base64,"));
}

#[tokio::test]
async fn builtin_css_skips_css_crawl() {
    let server = MockServer::start();

    for path in ["/avatar/120.png", "/img.png"] {
        server.mock(|when, then| {
            when.method(GET).path(path);
            then.status(200)
                .header("Content-Type", "image/png")
                .body(png_bytes());
        });
    }

    // Intentionally do NOT mock "/" or any CSS endpoints. If the renderer tries to auto-discover CSS
    // from base_url, it will fail this test.

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("topic.json");

    let base_url = Url::parse(&server.url("/")).unwrap();
    let topic_json = r#"{
  "id": 123,
  "title": "Test Topic",
  "post_stream": {
    "posts": [
      {
        "id": 1,
        "post_number": 1,
        "username": "alice",
        "display_username": "alice",
        "avatar_template": "/avatar/{size}.png",
        "created_at": "2026-01-30T00:00:00.000Z",
        "cooked": "<p>Hello</p><p><img src=\"/img.png\"></p>"
      }
    ]
  }
}"#;
    std::fs::write(&input, topic_json).unwrap();

    // dir mode (builtin css, no --css)
    let out_dir = tmp.path().join("out");
    let args = discourse_topic_render::CliArgs {
        input: input.clone(),
        base_url: base_url.clone(),
        css: vec![],
        builtin_css: true,
        mode: discourse_topic_render::Mode::Dir,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_dir.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html_path = out_dir.join("topic-123.html");
    let css_path = out_dir.join("assets/css/site.css");
    assert!(html_path.exists());
    assert!(css_path.exists());

    let html = read_to_string(&html_path);
    let css_out = read_to_string(&css_path);
    assert_no_remote_autoload(&html);
    assert!(html.contains("dtr-theme-toggle"));
    assert!(html.contains("class=\"dtr-post\""));
    assert!(css_out.contains(".dtr-post"));

    // single mode (builtin css, no --css)
    let out_single = tmp.path().join("topic-123-single.html");
    let args = discourse_topic_render::CliArgs {
        input,
        base_url,
        css: vec![],
        builtin_css: true,
        mode: discourse_topic_render::Mode::Single,
        offline: discourse_topic_render::OfflineMode::Strict,
        out: Some(out_single.clone()),
        avatar_size: 120,
        assets_dir_name: "assets".to_string(),
        max_concurrency: 4,
        user_agent: "test-agent".to_string(),
    };
    discourse_topic_render::run(args).await.unwrap();

    let html = read_to_string(&out_single);
    assert_no_remote_autoload(&html);
    assert!(html.contains("dtr-theme-toggle"));
    assert!(html.contains(".dtr-post"));
    assert!(html.contains("data:image/png;base64,"));
}
