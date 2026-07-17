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
//! だけのもの、`ai-tech-ranking`のような複数の異なる配列を持つもの、
//! `rakuten-*`系のようなスカラー値+日英併記フィールドの塊等)。
//! 2026-07-17、全8種のキャッシュに対応する**完全再帰の汎用レンダラー**
//! (`render_value_generic`)に書き換えた——形状ごとに専用コードを書く
//! のではなく、JSON構造から機械的にHTMLへ変換する(スカラー値は`<p>`、
//! 文字列配列は`<ul>`、同一キー構成のオブジェクト配列は表、
//! それ以外のオブジェクト配列・ネストしたオブジェクトは再帰)。

use poem::web::{Html, Path};
use poem::{get, handler, listener::TcpListener, Route, Server};
use serde_json::Value;

const CACHE_BASE: &str = "https://audiocafe.tokyo";
const ARUARU_TOKYO_URL: &str = "https://aruaru.tokyo/";

/// このRust側が対応済みのランキング一覧(表示名・キャッシュファイル名)。
/// 2026-07-17、汎用レンダラーへの書き換えにより全8種類に対応。
const RANKINGS: &[(&str, &str, &str)] = &[
    ("aruaru-caba", "aruaru-caba-ranking-cache.json", "キャバクラ求人 時給帯ランキング(地域別)"),
    ("aruaru-eikaiwa", "aruaru-eikaiwa-ranking-cache.json", "英会話スクール ランキング"),
    ("aruaru-jukujo-caba", "aruaru-jukujo-caba-ranking-cache.json", "熟女キャバ 求人ランキング"),
    ("ai-tech-ranking", "ai-tech-ranking-cache.json", "プログラミング言語・フレームワーク・DBランキング"),
    ("aruaru-learning-prices", "aruaru-learning-prices-cache.json", "学習サービス月額料金"),
    ("rakuten-mobile", "rakuten-mobile-cache.json", "楽天モバイル プラン情報"),
    ("rakuten-intl-call", "rakuten-intl-call-cache.json", "楽天モバイル 国際通話"),
    ("rakuten-platinum", "rakuten-platinum-cache.json", "楽天モバイル プラチナバンド・衛星"),
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
a {{ color: #222; }}
a:visited {{ color: #222; }}
nav a {{ margin-right: 1rem; }}
</style>
</head>
<body>
<nav><a href="/">TOP</a> <a href="/help">困った時は</a> <a href="{ARUARU_TOKYO_URL}">aruaru.tokyo</a></nav>
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

/// オブジェクト配列を、可能なら表として描画する。各要素が同じキー集合を
/// 持つ場合(このキャッシュ群の大半のケース)は列を揃えた表に、そうでない
/// 場合は要素ごとの箇条書き(キー:値)にフォールバックする。
fn render_object_array(items: &[Value]) -> String {
    let Some(first) = items.first().and_then(|v| v.as_object()) else {
        return String::new();
    };
    let columns: Vec<&String> = first.keys().collect();
    let uniform = items.iter().all(|v| {
        v.as_object().map(|o| {
            let keys: Vec<&String> = o.keys().collect();
            keys == columns
        }).unwrap_or(false)
    });

    if uniform {
        let mut out = String::from("<table><tr>");
        for col in &columns {
            out.push_str(&format!("<th>{}</th>", html_escape(col)));
        }
        out.push_str("</tr>");
        for item in items {
            out.push_str("<tr>");
            for col in &columns {
                let cell = item.get(col.as_str());
                let rendered = match cell {
                    Some(Value::String(s)) if s.starts_with("http://") || s.starts_with("https://") => {
                        format!(r#"<a href="{0}" target="_blank" rel="noopener noreferrer">🔎 リンク</a>"#, html_escape(s))
                    }
                    Some(Value::String(s)) => html_escape(s),
                    Some(v) => html_escape(&v.to_string()),
                    None => String::new(),
                };
                out.push_str(&format!("<td>{rendered}</td>"));
            }
            out.push_str("</tr>");
        }
        out.push_str("</table>");
        out
    } else {
        let mut out = String::from("<ul>");
        for item in items {
            if let Some(obj) = item.as_object() {
                let pairs: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("{}: {}", html_escape(k), html_escape(&v.to_string())))
                    .collect();
                out.push_str(&format!("<li>{}</li>", pairs.join(" / ")));
            }
        }
        out.push_str("</ul>");
        out
    }
}

/// JSON値を再帰的にHTMLへ変換する完全汎用レンダラー。8種類全てのキャッシュ
/// 形状(地域別ネスト・フラットrows・複数配列の塊・スカラー値+日英併記
/// フィールド)をこの1関数だけでカバーする。
fn render_value_generic(data: &Value, depth: u8) -> String {
    let mut out = String::new();
    let Some(obj) = data.as_object() else {
        return html_escape(&data.to_string());
    };

    // 先に見出し的なスカラーフィールド(updated_at/disclaimer等)を出す。
    for key in ["updated_at", "crawled_at", "disclaimer"] {
        if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
            out.push_str(&format!("<p class=\"disclaimer\">{}: {}</p>", html_escape(key), html_escape(v)));
        }
    }

    for (key, value) in obj {
        if matches!(key.as_str(), "updated_at" | "crawled_at" | "disclaimer") {
            continue;
        }
        match value {
            Value::String(s) if s.starts_with("http://") || s.starts_with("https://") => {
                out.push_str(&format!(
                    r#"<p><strong>{}:</strong> <a href="{}" target="_blank" rel="noopener noreferrer">🔎 リンク</a></p>"#,
                    html_escape(key), html_escape(s)
                ));
            }
            Value::String(s) => {
                out.push_str(&format!("<p><strong>{}:</strong> {}</p>", html_escape(key), html_escape(s)));
            }
            Value::Number(_) | Value::Bool(_) => {
                out.push_str(&format!("<p><strong>{}:</strong> {}</p>", html_escape(key), html_escape(&value.to_string())));
            }
            Value::Array(items) if items.iter().all(|v| v.is_string()) => {
                let list: String = items.iter().map(|v| format!("<li>{}</li>", html_escape(v.as_str().unwrap_or("")))).collect();
                out.push_str(&format!("<h{n}>{k}</h{n}><ul>{list}</ul>", n = (depth + 2).min(6), k = html_escape(key)));
            }
            Value::Array(items) if !items.is_empty() && items.iter().all(|v| v.is_object()) => {
                out.push_str(&format!("<h{n}>{k}</h{n}>", n = (depth + 2).min(6), k = html_escape(key)));
                out.push_str(&render_object_array(items));
            }
            Value::Object(_) => {
                out.push_str(&format!("<h{n}>{k}</h{n}>", n = (depth + 2).min(6), k = html_escape(key)));
                out.push_str(&render_value_generic(value, depth + 1));
            }
            _ => {}
        }
    }
    out
}

fn render_ranking_body(label: &str, data: &Value) -> String {
    format!("<h1>{}</h1>{}", html_escape(label), render_value_generic(data, 0))
}

/// PHP側の`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`サブページは、
/// 複数のキャッシュファイルを1ページに束ねて表示する構成になっている
/// (調査済み、2026-07-17)。同じ構成をRust側でも複合ページとして再現する。
/// キャッシュパスは`CACHE_BASE`からの相対パス(サブディレクトリ込み)。
struct CompositePage {
    slug: &'static str,
    title: &'static str,
    sections: &'static [(&'static str, &'static str)], // (見出し, キャッシュの相対パス)
}

const COMPOSITE_PAGES: &[CompositePage] = &[
    CompositePage {
        slug: "rakuten-mobile",
        title: "楽天モバイル 総合ページ",
        sections: &[
            ("基本プラン", "rakuten-mobile-cache.json"),
            ("国際通話", "rakuten-intl-call-cache.json"),
            ("プラチナバンド・衛星", "rakuten-platinum-cache.json"),
        ],
    },
    CompositePage {
        slug: "aruaru",
        title: "aruaru(IT・建築系求人 総合ページ)",
        sections: &[
            ("キャバクラ求人 時給帯ランキング", "aruaru-caba-ranking-cache.json"),
            ("熟女キャバ 求人ランキング", "aruaru-jukujo-caba-ranking-cache.json"),
            ("英会話スクール ランキング", "aruaru-eikaiwa-ranking-cache.json"),
            ("学習サービス月額料金", "aruaru-learning-prices-cache.json"),
            ("プログラミング言語・フレームワーク・DBランキング", "ai-tech-ranking-cache.json"),
            ("楽天モバイル 基本プラン", "aruaru/rakuten-mobile-cache.json"),
            ("楽天モバイル 国際通話", "aruaru/rakuten-intl-call-cache.json"),
            ("楽天モバイル プラチナバンド・衛星", "aruaru/rakuten-platinum-cache.json"),
            ("楽天モバイル スマホキャンペーン", "aruaru/rakuten-smartphone-cache.json"),
            ("doda 求人情報", "aruaru/doda-jobs-cache.json"),
        ],
    },
    CompositePage {
        slug: "aruaru-lady",
        title: "aruaru-lady(女性向け求人 総合ページ)",
        sections: &[
            ("キャバクラ求人 時給帯ランキング", "aruaru-lady/aruaru-caba-ranking-cache.json"),
            ("熟女キャバ 求人ランキング", "aruaru-lady/aruaru-jukujo-caba-ranking-cache.json"),
            ("TVチャット(通常) ランキング", "aruaru-lady/aruaru-tvchat-normal-ranking-cache.json"),
            ("TVチャット(グループ) ランキング", "aruaru-lady/aruaru-tvchat-group-ranking-cache.json"),
            ("楽天モバイル 基本プラン", "aruaru-lady/rakuten-mobile-cache.json"),
            ("楽天モバイル 国際通話", "aruaru-lady/rakuten-intl-call-cache.json"),
            ("楽天モバイル プラチナバンド・衛星", "aruaru-lady/rakuten-platinum-cache.json"),
        ],
    },
];

async fn render_composite_body(page: &CompositePage) -> String {
    let mut out = format!("<h1>{}</h1>", html_escape(page.title));
    for (heading, path) in page.sections {
        out.push_str(&format!("<h2>{}</h2>", html_escape(heading)));
        match fetch_cache(path).await {
            Ok(data) => out.push_str(&render_value_generic(&data, 1)),
            Err(e) => out.push_str(&format!("<p class=\"disclaimer\">取得エラー: {}</p>", html_escape(&e))),
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
    let composite_list: String = COMPOSITE_PAGES
        .iter()
        .map(|p| format!(r#"<li><a href="/page/{}">{}</a></li>"#, p.slug, html_escape(p.title)))
        .collect();
    let body = format!(
        r#"<h1>audiocafe.tokyo (Rust版、移行中)</h1>
<p>既存PHPサイトのジャンル別ランキング表示を、段階的にRust + Poemへ移行しています。</p>

<h2>総合ページ(既存PHP側の/aruaru・/aruaru-lady・/rakuten-mobileに相当)</h2>
<ul>{composite_list}</ul>

<h2>個別ランキング</h2>
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

#[handler]
async fn composite_page(Path(slug): Path<String>) -> Html<String> {
    let Some(page) = COMPOSITE_PAGES.iter().find(|p| p.slug == slug) else {
        return Html(page_shell("見つかりません", "<h1>404</h1><p>未対応のページです。</p>"));
    };
    Html(page_shell(page.title, &render_composite_body(page).await))
}

#[handler]
fn help_page() -> Html<String> {
    let body = r#"<h1>困った時は</h1>

<h2>Google Chromeで「保護されていない通信」と出る場合</h2>
<p>Edge(Windowsの証明書ストアを使用)では正常なのに対し、Chromeは独自の
「Chrome Root Store」という、Windowsとは別の信頼済みルート証明書リストを
持っています。新しいLet's Encryptのルート証明書がまだお使いのChromeの
バージョンに反映されていない可能性があります。</p>
<p><strong>対処法:</strong> Chromeを<code>chrome://settings/help</code>から更新し、
再起動(タスクマネージャーでプロセスが残っていないか確認)してから再度アクセスしてください。</p>

<h2>サイトが表示されない場合(DNS_PROBE_FINISHED_NXDOMAIN等)</h2>
<p>お使いのDNS(特にCloudflareの1.1.1.1)が、ドメインの権威サーバーに
一時的に到達できないことがあります。Google(8.8.8.8)・Quad9(9.9.9.9)
など別のDNSでは問題なく解決できることが多いです。</p>
<p><strong>対処法:</strong> スマホのモバイル回線(Wi-Fiオフ)で試すか、
Windowsの設定(ネットワークとインターネット → プロパティ → DNSサーバーの
割り当てを「手動」)でDNSサーバーを変更してください。
<strong>優先DNS</strong>欄に<code>8.8.8.8</code>のみ、<strong>代替DNS</strong>欄に
<code>8.8.4.4</code>をそれぞれ別々に入力してください
(1つの欄に<code>8.8.8.8 / 8.8.4.4</code>とまとめて入力すると
「無効なエントリ」エラーになります)。「HTTPS経由のDNS」が
「オン(手動テンプレート)」の場合はまず「オフ」にしてから保存を試してください。
スマホでWi-Fi経由の場合は、静的IP化不要で「プライベートDNS」設定だけ
変更できます(設定 → ネットワークとインターネット、機種によっては
Wi-Fi → 詳細設定の中にある場合も → 「プライベートDNS」→
「プロバイダのホスト名」を選択 → <code>dns.google</code> と入力して保存)。
それでも解決しない場合は、単純にDNSの反映待ち(通常数分〜1時間程度)
であることも多いです。</p>
"#;
    Html(page_shell("困った時は", body))
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt::init();
    let app = Route::new()
        .at("/", get(top))
        .at("/healthz", get(healthz))
        .at("/help", get(help_page))
        .at("/ranking/:slug", get(ranking_page))
        .at("/page/:slug", get(composite_page));

    tracing::info!("audiocafe-tokyo-server listening on 127.0.0.1:4400");
    Server::new(TcpListener::bind("127.0.0.1:4400")).run(app).await
}
