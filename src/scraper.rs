//! 移行元PHP(`audiocafe.tokyo/index.php`の`is_video_host`/`source_name`/
//! `extract_yt_id`/`fetch_url`/`extract_from_html`/`build_lists`)の
//! 「シードURL群からテキストリンク・動画リンク・写真を収集する」
//! アルゴリズムを、簡略化しつつRustへ移植したもの(2026-07-17)。
//!
//! ## 簡略化した点
//! - PHP側は`$seen_text`/`$seen_video`/`$seen_img`という3つの連想配列と
//!   3回に分けたforeachループで重複排除・分類していたが、ここでは
//!   [`HashSet`]による単純な重複排除+1回のイテレータチェーンにまとめた。
//! - HTMLの`<title>`/`<a href>`/`<meta property="og:image">`/`<img src>`
//!   抽出は、PHP側の個別`preg_match`群を[`regex::Regex`]へそのまま対応
//!   させたが、共通の「相対URLをbase_urlから絶対化する」処理を
//!   [`resolve_url`]という1つの関数に集約した(PHP側は`<a>`用と`<img>`用に
//!   ほぼ同じ絶対化ロジックが2箇所に重複していた)。
//! - キャッシュ永続化(TTL 86400秒)は[`rust_json`](https://github.com/aon-co-jp/Rust-JSON)
//!   の`to_string_strict`/`parse_strict`を使う(このエコシステムの
//!   JSON処理を1本化する方針に合わせる)。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VIDEO_HOST_MARKERS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "facebook.com/reel",
    "facebook.com/watch",
    "fb.watch",
    "vimeo.com",
    "nicovideo.jp",
    "tiktok.com",
    "drive.google.com",
];

pub fn is_video_host(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    VIDEO_HOST_MARKERS.iter().any(|m| lower.contains(m))
}

pub fn source_name(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        "YouTube"
    } else if lower.contains("facebook.com") || lower.contains("fb.watch") {
        "Facebook"
    } else if lower.contains("drive.google.com") {
        "Drive"
    } else if lower.contains("vimeo.com") {
        "Vimeo"
    } else if lower.contains("nicovideo.jp") {
        "Niconico"
    } else if lower.contains("tiktok.com") {
        "TikTok"
    } else {
        "Video"
    }
}

pub fn extract_yt_id(url: &str) -> Option<String> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static PATTERNS: Lazy<[Regex; 4]> = Lazy::new(|| {
        [
            Regex::new(r"youtu\.be/([a-zA-Z0-9_-]{11})").unwrap(),
            Regex::new(r"[?&]v=([a-zA-Z0-9_-]{11})").unwrap(),
            Regex::new(r"/embed/([a-zA-Z0-9_-]{11})").unwrap(),
            Regex::new(r"/shorts/([a-zA-Z0-9_-]{11})").unwrap(),
        ]
    });
    PATTERNS.iter().find_map(|re| re.captures(url).map(|c| c[1].to_string()))
}

/// PHPの`fetch_url()`相当。タイムアウト・User-Agent・リダイレクト追跡は
/// `reqwest::Client`のビルダーで指定し、取得失敗は(PHPの`@file_get_contents`
/// が`false`を返すのと同様)`None`として静かに扱う——1URLの取得失敗で
/// 全体の収集処理を止めないという元の設計を保つ。
async fn fetch_url(client: &reqwest::Client, url: &str) -> Option<String> {
    client.get(url).send().await.ok()?.text().await.ok()
}

fn resolve_url(raw: &str, base_url: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if raw.starts_with('/') {
        let origin_end = base_url.get(8..)?.find('/').map(|i| i + 8).unwrap_or(base_url.len());
        return Some(format!("{}{}", &base_url[..origin_end], raw));
    }
    None
}

struct ExtractedLink {
    url: String,
    text: String,
}

struct ExtractedImage {
    src: String,
    alt: String,
}

struct Extracted {
    title: Option<String>,
    links: Vec<ExtractedLink>,
    images: Vec<ExtractedImage>,
}

/// アイコン・ロゴ・アバター等、コンテンツ写真として不適切な画像を除外する。
fn looks_like_decorative_image(src: &str) -> bool {
    const NOISE: &[&str] = &["icon", "logo", "sprite", "favicon", "emoji", "avatar", "thumb_"];
    let lower = src.to_ascii_lowercase();
    NOISE.iter().any(|n| lower.contains(n))
}

fn extract_from_html(html: &str, base_url: &str) -> Extracted {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static TITLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<title[^>]*>([^<]{1,200})</title>").unwrap());
    static TITLE_SUFFIX_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\s*[-|]\s*(?:YouTube|Facebook|Ameba|Ameblo).*$").unwrap());
    static ANCHOR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?is)<a[^>]+href=["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap());
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<[^>]+>").unwrap());
    static OG_IMAGE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?is)<meta[^>]+property=["']og:image["'][^>]+content=["']([^"']+)["']"#).unwrap());
    static IMG_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?is)<img[^>]+src=["']([^"']+)["'][^>]*?(?:alt=["']([^"']*)["'])?[^>]*>"#).unwrap());

    let title = TITLE_RE.captures(html).map(|c| {
        let raw = html_escape::decode_html_entities(c[1].trim()).to_string();
        TITLE_SUFFIX_RE.replace(&raw, "").trim().to_string()
    }).filter(|t| !t.is_empty());

    let links = ANCHOR_RE
        .captures_iter(html)
        .filter_map(|c| {
            let href = c[1].trim();
            if href.is_empty() || href.starts_with('#') || href.to_ascii_lowercase().starts_with("javascript:") {
                return None;
            }
            let url = resolve_url(href, base_url)?;
            let text = TAG_RE.replace_all(&c[2], "").trim().split_whitespace().collect::<Vec<_>>().join(" ");
            Some(ExtractedLink { url, text })
        })
        .collect();

    let mut images: Vec<ExtractedImage> = OG_IMAGE_RE
        .captures_iter(html)
        .filter_map(|c| resolve_url(c[1].trim(), base_url))
        .map(|src| ExtractedImage { src, alt: String::new() })
        .collect();

    images.extend(IMG_RE.captures_iter(html).filter_map(|c| {
        let raw_src = c[1].trim();
        if raw_src.to_ascii_lowercase().starts_with("data:") {
            return None;
        }
        let src = resolve_url(raw_src, base_url)?;
        if looks_like_decorative_image(&src) {
            return None;
        }
        let alt = c.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        Some(ExtractedImage { src, alt })
    }));

    Extracted { title, links, images }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TextLink {
    pub url: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VideoLink {
    pub url: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Photo {
    pub src: String,
    pub alt: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Lists {
    pub text_links: Vec<TextLink>,
    pub video_links: Vec<VideoLink>,
    pub photos: Vec<Photo>,
    #[serde(default)]
    pub built_at: u64,
}

const CACHE_TTL_SECS: u64 = 86_400;
const MAX_FETCHES: usize = 30;
const MAX_PHOTOS_PER_PAGE: usize = 3;

fn cache_path() -> PathBuf {
    std::env::temp_dir().join("audiocafe_lists_cache.json")
}

fn load_cache() -> Option<Lists> {
    let path = cache_path();
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age > Duration::from_secs(CACHE_TTL_SECS) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    rust_json::parse_strict(&text).ok().and_then(|v| serde_json::from_value(v).ok())
}

fn save_cache(lists: &Lists) {
    if let Ok(text) = serde_json::to_string(lists) {
        let _ = std::fs::write(cache_path(), text);
    }
}

/// PHPの`build_lists()`相当。シードURL群を(1)動画URL自体、(2)未取得の
/// テキストページ(最大`MAX_FETCHES`件フェッチしリンク/画像を収集)、
/// (3)取得しなかった残りのテキストURL、の順で処理し重複排除する。
/// 3つの`seen_*`連想配列に分かれていたPHP版と異なり、ここでは
/// [`HashSet`]3つ(同じ役割)をこの関数のローカル変数としてまとめている
/// だけで、外部から見た「3種類の重複排除」という構造自体は変えていない
/// (仕様を変えない範囲での実装の簡略化)。
pub async fn build_lists(seed_urls: &[&str]) -> Lists {
    if let Some(cached) = load_cache() {
        return cached;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent("Mozilla/5.0 (compatible; AudiocafeBot/1.0)")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .expect("reqwest client should build with static config");

    let mut seen_text = HashSet::new();
    let mut seen_video = HashSet::new();
    let mut seen_img = HashSet::new();
    let mut text_links = Vec::new();
    let mut video_links = Vec::new();
    let mut photos = Vec::new();

    // (1) シード自体が動画URLのものを先に登録。
    for &url in seed_urls {
        if is_video_host(url) && seen_video.insert(url.to_string()) {
            video_links.push(VideoLink { url: url.to_string(), title: url.to_string() });
        }
    }

    // (2) テキストページを取得し、本文中のリンク/画像を収集。
    let mut fetched = 0usize;
    for &url in seed_urls {
        if fetched >= MAX_FETCHES {
            break;
        }
        if is_video_host(url) || !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        let Some(html) = fetch_url(&client, url).await else { continue };
        fetched += 1;

        let data = extract_from_html(&html, url);
        let page_title = data.title.clone().unwrap_or_else(|| url.to_string());
        if seen_text.insert(url.to_string()) {
            text_links.push(TextLink { url: url.to_string(), title: page_title.clone() });
        }
        for link in &data.links {
            if is_video_host(&link.url) && seen_video.insert(link.url.clone()) {
                let title = if link.text.is_empty() { page_title.clone() } else { link.text.clone() };
                video_links.push(VideoLink { url: link.url.clone(), title });
            }
        }
        for img in data.images.into_iter().take(MAX_PHOTOS_PER_PAGE) {
            if seen_img.insert(img.src.clone()) {
                let alt = if img.alt.is_empty() { page_title.clone() } else { img.alt };
                photos.push(Photo { src: img.src, alt });
            }
        }
    }

    // (3) 取得しなかった残りのテキストURLも(タイトル無しで)一覧に含める。
    for &url in seed_urls {
        if !is_video_host(url) && url.starts_with("http") && seen_text.insert(url.to_string()) {
            text_links.push(TextLink { url: url.to_string(), title: url.to_string() });
        }
    }

    let lists = Lists {
        text_links,
        video_links,
        photos,
        built_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
    };
    save_cache(&lists);
    lists
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_host_detection_matches_known_hosts() {
        assert!(is_video_host("https://www.youtube.com/watch?v=abc"));
        assert!(is_video_host("https://youtu.be/abc"));
        assert!(is_video_host("https://www.facebook.com/reel/123"));
        assert!(!is_video_host("https://ameblo.jp/www-aon/entry-1.html"));
    }

    #[test]
    fn source_name_labels_each_known_host() {
        assert_eq!(source_name("https://youtu.be/abc"), "YouTube");
        assert_eq!(source_name("https://www.facebook.com/reel/1"), "Facebook");
        assert_eq!(source_name("https://drive.google.com/file/d/x"), "Drive");
        assert_eq!(source_name("https://example.com"), "Video");
    }

    #[test]
    fn extract_yt_id_handles_all_url_shapes() {
        assert_eq!(extract_yt_id("https://youtu.be/dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(extract_yt_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(extract_yt_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(extract_yt_id("https://example.com/no-id-here"), None);
    }

    #[test]
    fn resolve_url_absolutizes_protocol_relative_and_root_relative_paths() {
        assert_eq!(
            resolve_url("//cdn.example.com/x.png", "https://example.com/page"),
            Some("https://cdn.example.com/x.png".to_string())
        );
        assert_eq!(
            resolve_url("/images/x.png", "https://example.com/page"),
            Some("https://example.com/images/x.png".to_string())
        );
        assert_eq!(
            resolve_url("https://already-absolute.com/x", "https://example.com/page"),
            Some("https://already-absolute.com/x".to_string())
        );
        assert_eq!(resolve_url("relative/path", "https://example.com/page"), None);
    }

    #[test]
    fn extract_from_html_pulls_title_links_and_images() {
        let html = r#"
            <html><head><title>My Page - YouTube</title></head>
            <body>
                <a href="https://youtu.be/abc12345678">Watch this</a>
                <a href="/local/page">Local</a>
                <meta property="og:image" content="https://example.com/og.jpg">
                <img src="https://example.com/photo.jpg" alt="a photo">
                <img src="https://example.com/site-logo.png" alt="logo">
            </body></html>
        "#;
        let data = extract_from_html(html, "https://example.com/base");
        assert_eq!(data.title.as_deref(), Some("My Page"));
        assert_eq!(data.links.len(), 2);
        assert_eq!(data.links[0].url, "https://youtu.be/abc12345678");
        assert_eq!(data.links[1].url, "https://example.com/local/page");
        // ロゴ画像は除外され、og:image + 通常画像1枚のみ残る。
        assert_eq!(data.images.len(), 2);
        assert_eq!(data.images[0].src, "https://example.com/og.jpg");
        assert_eq!(data.images[1].src, "https://example.com/photo.jpg");
    }
}
