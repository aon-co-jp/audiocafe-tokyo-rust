//! audiocafe-tokyo-server — audiocafe.tokyo のPHPモノリス(index.php、
//! 445KB・189関数)をRust+Poemへ段階的に移行するための新サイト。
//! aruaru-tokyo-server/aon-tokyo/karu-tokyo と同じ技術スタック・実装方針
//! (DB非依存・1バイナリ完結・テンプレートエンジン不使用)を踏襲する。
//!
//! ## 移行方針(2026-07-17、第一段)
//! 既存PHP側はDB接続をほとんど持たず、実データの大半は
//! `*-cache.json`(ファイルベースキャッシュ、`https://audiocafe.tokyo/`
//! 直下に静的公開済み)によるジャンル別ランキング表示。このRust側は
//! そのキャッシュJSONをHTTP経由で取得し(サーバー間の直接ファイル共有を
//! 前提にしない、疎結合)、[`rust_json`](https://github.com/aon-co-jp/Rust-JSON)
//! (`parse_strict`)でパースして汎用的にレンダリングする。
//!
//! キャッシュのスキーマは完全には統一されていない(地域別
//! `tokyo_23`/`tokyo_tama`/`national`に分かれるもの、フラットな`rows`
//! だけのもの等)。今回は代表的な2形状(地域別・フラット`rows`)を汎用
//! レンダラーで対応し、`ai-tech-ranking`・`aruaru-learning-prices`・
//! `rakuten-*`系(さらに異なるネスト構造)は次回以降の移行対象として
//! `CLAUDE.md`のHANDOFFに正直に記録する(未対応をここでごまかさない)。

use poem::web::{Html, Path};
use poem::{get, handler, listener::TcpListener, Route, Server};
use serde_json::Value;

const CACHE_BASE: &str = "https://audiocafe.tokyo";
const ARUARU_TOKYO_URL: &str = "https://aruaru.tokyo/";

/// このRust側が対応済みのランキング一覧(表示名・キャッシュファイル名)。
/// 未対応のキャッシュ(ai-tech-ranking等)はここに含めない。
const RANKINGS: &[(&str, &str, &str)] = &[
    ("aruaru-caba", "aruaru-caba-ranking-cache.json", "キャバクラ求人 時給帯ランキング(地域別)"),
    ("aruaru-eikaiwa", "aruaru-eikaiwa-ranking-cache.json", "英会話スクール ランキング"),
    ("aruaru-jukujo-caba", "aruaru-jukujo-caba-ranking-cache.json", "熟女キャバ 求人ランキング"),
];

fn page_shell(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
body {{ font-family: -apple-system, "Hiragino Sans", "Yu Gothic", sans-serif; max-width: 900px; margin: 2rem auto; padding: 0 1rem; line-height: 1.7; color: #222; }}
h1 {{ font-size: 1.6rem; }}
h2 {{ font-size: 1.15rem; margin-top: 2rem; border-bottom: 2px solid #eee; padding-bottom: 0.3rem; }}
table {{ border-collapse: collapse; width: 100%; margin: 1rem 0; }}
th, td {{ border: 1px solid #ddd; padding: 0.4rem 0.6rem; text-align: left; font-size: 0.92rem; }}
th {{ background: #f5f5f5; }}
.disclaimer {{ font-size: 0.8rem; color: #777; }}
nav a {{ margin-right: 1rem; }}
</style>
</head>
<body>
<nav><a href="/">TOP</a> <a href="{ARUARU_TOKYO_URL}">aruaru.tokyo</a></nav>
{body}
</body>
</html>"#
    )
}

async fn fetch_cache(filename: &str) -> Result<Value, String> {
    let url = format!("{CACHE_BASE}/{filename}");
    let text = reqwest::get(&url).await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    rust_json::parse_strict(&text).map_err(|e| e.to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// フラットな`rows`配列を表として描画。
fn render_rows_table(rows: &[Value]) -> String {
    let mut out = String::from("<table><tr><th>順位</th><th>項目</th><th>詳細</th><th>リンク</th></tr>");
    for row in rows {
        let rank = row.get("rank").map(|v| v.to_string()).unwrap_or_default();
        let title = row.get("title").or_else(|| row.get("area")).and_then(|v| v.as_str()).unwrap_or("");
        let band = row.get("band").and_then(|v| v.as_str()).unwrap_or("");
        let pickup = row.get("pickup").and_then(|v| v.as_str()).unwrap_or("");
        let url = row.get("url").and_then(|v| v.as_str());
        let link = url
            .map(|u| format!(r#"<a href="{}" target="_blank" rel="noopener noreferrer">🔎 詳細を見る</a>"#, html_escape(u)))
            .unwrap_or_default();
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}<br><small>{}</small></td><td>{}</td><td>{}</td></tr>",
            html_escape(&rank),
            html_escape(title),
            html_escape(band),
            html_escape(pickup),
            link
        ));
    }
    out.push_str("</table>");
    out
}

/// 対応する2形状(地域別/フラット)を判定して汎用レンダリング。
fn render_ranking_body(label: &str, data: &Value) -> String {
    let mut out = format!("<h1>{}</h1>", html_escape(label));
    if let Some(updated_at) = data.get("updated_at").and_then(|v| v.as_str()) {
        out.push_str(&format!("<p class=\"disclaimer\">更新日時: {}</p>", html_escape(updated_at)));
    }
    if let Some(disclaimer) = data.get("disclaimer").and_then(|v| v.as_str()) {
        out.push_str(&format!("<p class=\"disclaimer\">{}</p>", html_escape(disclaimer)));
    }

    // フラット `rows` 形状(aruaru-eikaiwa / aruaru-jukujo-caba)。
    if let Some(rows) = data.get("rows").and_then(|v| v.as_array()) {
        out.push_str(&render_rows_table(rows));
        return out;
    }

    // 地域別形状(aruaru-caba: tokyo_23 / tokyo_tama / national、各 {rows: [...]})。
    const REGION_LABELS: &[(&str, &str)] = &[
        ("tokyo_23", "東京23区"),
        ("tokyo_tama", "東京多摩地域"),
        ("national", "全国"),
    ];
    for (key, region_label) in REGION_LABELS {
        if let Some(rows) = data.get(*key).and_then(|v| v.get("rows")).and_then(|v| v.as_array()) {
            out.push_str(&format!("<h2>{}</h2>", html_escape(region_label)));
            out.push_str(&render_rows_table(rows));
        }
    }
    out
}

#[handler]
fn healthz() -> &'static str {
    "ok"
}

#[handler]
fn top() -> Html<String> {
    let list: String = RANKINGS
        .iter()
        .map(|(slug, _, label)| format!(r#"<li><a href="/ranking/{slug}">{}</a></li>"#, html_escape(label)))
        .collect();
    let body = format!(
        r#"<h1>audiocafe.tokyo (Rust版、移行中)</h1>
<p>既存PHPサイトのジャンル別ランキング表示を、段階的にRust + Poemへ移行しています。
現時点で対応済みのランキングは以下の通りです(未対応分は既存PHP側でご覧いただけます)。</p>
<ul>{list}</ul>
"#
    );
    Html(page_shell("audiocafe.tokyo (Rust移行版)", &body))
}

#[handler]
async fn ranking_page(Path(slug): Path<String>) -> Html<String> {
    let Some((_, filename, label)) = RANKINGS.iter().find(|(s, _, _)| *s == slug) else {
        return Html(page_shell("見つかりません", "<h1>404</h1><p>未対応のランキングです。</p>"));
    };
    match fetch_cache(filename).await {
        Ok(data) => Html(page_shell(label, &render_ranking_body(label, &data))),
        Err(e) => Html(page_shell("エラー", &format!("<h1>取得エラー</h1><p>{}</p>", html_escape(&e)))),
    }
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt::init();
    let app = Route::new()
        .at("/", get(top))
        .at("/healthz", get(healthz))
        .at("/ranking/:slug", get(ranking_page));

    tracing::info!("audiocafe-tokyo-server listening on 127.0.0.1:4400");
    Server::new(TcpListener::bind("127.0.0.1:4400")).run(app).await
}
