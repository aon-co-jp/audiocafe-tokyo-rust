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

mod cron;
mod scraper;
mod seed_urls;

use open_runo_router::hyper_compat::{self, Params};
use hyper::{Method, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use once_cell::sync::Lazy;
use serde::Deserialize;

const CACHE_BASE: &str = "https://audiocafe.tokyo";
const ARUARU_TOKYO_URL: &str = "https://aruaru.tokyo/";
/// 東京都西部の暮らし・テレワーク紹介 + open-cosmoエコシステムの入口
/// (2026-07-20追記、ユーザー指示: 「aruaru.tokyo と runo.tokyo へのリンクを
/// audiocafe.tokyo内にも貼って」)。
const RUNO_TOKYO_URL: &str = "https://runo.tokyo/";

/// ユーザー提供のブログ記事(タイトルをリンクテキストにし、URLそのものは
/// 表示しない、2026-07-20追記)。トップページのYouTube紹介タイトルの
/// 上に配置する(ユーザー指示)。
const BLOG_POST_URL: &str = "https://ameblo.jp/www-aon/entry-12973252437.html";
const BLOG_POST_TITLE_JA: &str = "プログラム言語やフレームワークなどの全てをRust(Poemやhyper)版に移植するメリット?";
/// ユーザー指示(2026-07-20)により日英両方で掲載。ブログ本文自体は
/// 日本語のみだが、リンクのラベルは英語話者にも内容が伝わるよう
/// 意訳したもの(URLは日英共通、リンク先は変えない)。
const BLOG_POST_TITLE_EN: &str = "The benefits of migrating everything — programming languages, frameworks, and more — to Rust";

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
.nav-php-link {{ font-size: 1.4em; font-weight: 800; }}
.site-header-band {{ background: #000; color: #fff; width: 100vw; margin: -2rem calc(-50vw + 50%) 0; padding: 1rem 1rem 0.8rem; box-sizing: border-box; }}
.site-header-band nav a {{ color: #7dd3fc; }}
.site-header-band .nav-php-link {{ color: #fbbf24 !important; }}
.site-header-band .ac-credit {{ color: #fff; background: #000; font-size: 0.85rem; margin-top: 0.5rem; }}
.site-header-band .ac-credit a {{ color: #7dd3fc; }}
</style>
</head>
<body>
<div class="site-header-band">
<nav><a href="/">TOP</a> <a href="/discover">Discover</a> <a href="/help">困った時は</a> <a href="{ARUARU_TOKYO_URL}">aruaru.tokyo</a> <a href="{RUNO_TOKYO_URL}">runo.tokyo</a> <a href="https://karu.tokyo/" target="_blank" rel="noopener noreferrer">karu.tokyo</a> <a href="/index.php" class="nav-php-link" target="_blank" rel="noopener noreferrer">PHP</a></nav>
<p class="ac-credit">Claude Code DESKTOPというAIに、ITスキルがほとんど無くてもアプリやWEBサイトが作れる技術で、PHP版をRust＋RPoem
（audiocafe.tokyoのRustへの移植プロジェクト:
<a href="https://github.com/aon-co-jp/audiocafe-tokyo-rust" target="_blank" rel="noopener noreferrer">audiocafe-tokyo-rust</a>）
へ日本語で命令して、移植が成功致しました。関連プロジェクト:
<a href="https://github.com/aon-co-jp/RTypeScript" target="_blank" rel="noopener noreferrer">RTypeScript</a>・
<a href="https://github.com/aon-co-jp/RPoem" target="_blank" rel="noopener noreferrer">RPoem</a>・
<a href="https://github.com/aon-co-jp/open-web-server" target="_blank" rel="noopener noreferrer">open-web-server</a>・
<a href="https://github.com/aon-co-jp/RFrontEnd" target="_blank" rel="noopener noreferrer">RFrontEnd</a>・
<a href="https://github.com/aon-co-jp/RReact" target="_blank" rel="noopener noreferrer">RReact</a></p>
</div>
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

/// JSON文字列フィールドを取得し、無ければ`default`を返す(PHP側の
/// `$_rm_rk['price'] ?? '最大3,278円（税込）'`等のnull合体演算子と同じ挙動)。
fn get_str(v: &Value, key: &str, default: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or(default).to_string()
}

fn get_bool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
}

/// PHPの`rawurlencode()`相当(Google検索リンク生成用)。UTF-8バイト単位で
/// 非予約文字を`%XX`に置き換える単純な実装。
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn google_search_url(query: &str) -> String {
    format!("https://www.google.com/search?q={}", percent_encode(query))
}

/// JSON値を種類を問わず表示用文字列にする(`get_str`は文字列型専用のため、
/// ランキングキャッシュの`rank`のような数値フィールドは拾えない。
/// aruaru-ladyのランキング表描画で数値・文字列混在フィールドを
/// まとめて扱うために追加)。
fn get_disp(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// aruaru-ladyの各種TOP50ランキング(キャバクラ/熟女キャバ/TVチャットレディ
/// グループ・通常)を、PHP版と同じ列構成(順位+指定カラム+検索リンク)の
/// 表としてレンダリングする。`cols`は(JSONキー, 表示ラベル)のペア。
fn render_rank_table(rows: &[Value], head_color: &str, cols: &[(&str, &str)]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::from(r#"<div class="ox"><table><thead><tr>"#);
    out.push_str(&format!(r#"<th style="width:48px;color:{head_color};">順位</th>"#));
    for (_, label) in cols {
        out.push_str(&format!(r#"<th style="color:{head_color};">{}</th>"#, html_escape(label)));
    }
    out.push_str(&format!(r#"<th style="color:{head_color};width:110px;">検索</th></tr></thead><tbody>"#));
    for row in rows {
        out.push_str(r#"<tr style="border-bottom:1px solid #4c1d45;">"#);
        out.push_str(&format!(
            r#"<td style="padding:6px 8px;text-align:center;font-weight:bold;color:{head_color};">{}</td>"#,
            html_escape(&get_disp(row, "rank"))
        ));
        for (key, _) in cols {
            out.push_str(&format!(r#"<td style="padding:6px 8px;">{}</td>"#, html_escape(&get_disp(row, key))));
        }
        let url = get_disp(row, "url");
        out.push_str(&format!(
            r#"<td style="padding:6px 8px;"><a href="{}" target="_blank" rel="noopener noreferrer" style="color:{head_color};font-weight:bold;">Google で検索</a></td>"#,
            html_escape(&url)
        ));
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    out
}

/// PHP版`aruaru-lady/index.php`(2916行)の`<style>`ブロック
/// (1424〜1449行目、および1450〜1469行目の一部)から、レイアウトの
/// 核となるダークテーマCSSを移植したもの。Google翻訳ウィジェット/
/// OPEN・CLOSEパネルのJS演出用CSS(1470行目以降)は対象外
/// (今回のスコープは見た目の一致であり、翻訳ウィジェット自体は
/// 別機能のため)。`page_shell`のヘッダー内`<style>`より後(body内)に
/// 出力することで、同名セレクタ(`body`・`a`・`table`等)を上書きする。
const ARUARU_LADY_STYLE: &str = r#"<style>
.aruaru-lady-page{margin:-2rem -1rem;background:#0a0f1e;color:#e2e8f0;font-family:'Helvetica Neue',Arial,'Hiragino Kaku Gothic ProN',sans-serif;line-height:1.7}
.aruaru-lady-page a{color:#fda4af;text-decoration:none}
.aruaru-lady-page a:hover{text-decoration:underline}
.aruaru-lady-page .wrap{max-width:1200px;margin:0 auto;padding:16px}
.aruaru-lady-page .hero{background:linear-gradient(135deg,#1a0a14 0%,#2d0a1e 50%,#1a0a14 100%);padding:40px 20px;text-align:center;border-bottom:2px solid #be185d}
.aruaru-lady-page .hero h1{font-size:clamp(22px,5vw,36px);color:#fda4af;font-weight:900;line-height:1.3;margin-bottom:12px}
.aruaru-lady-page .hero p{color:#fcd7e0;font-size:15px;opacity:.9;max-width:700px;margin:0 auto}
.aruaru-lady-page .notice{background:rgba(30,58,138,.25);border:1.5px solid #3b82f6;border-radius:12px;padding:12px 16px;margin:20px 0;font-size:15px;color:#bfdbfe;display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.aruaru-lady-page .notice a{color:#93c5fd;font-weight:700;text-decoration:underline}
.aruaru-lady-page .card{background:#110818;border:1px solid #4c1d45;border-radius:16px;padding:24px;margin:28px 0}
.aruaru-lady-page h2{font-size:clamp(18px,4vw,24px);color:#fda4af;margin-bottom:12px;line-height:1.35}
.aruaru-lady-page h3{font-size:clamp(15px,3.5vw,19px);color:#f472b6;margin:24px 0 10px}
.aruaru-lady-page table{width:100%;border-collapse:collapse;font-size:15px}
.aruaru-lady-page thead tr{background:#2d0a1e;text-align:left}
.aruaru-lady-page th{padding:8px 6px;color:#fda4af;white-space:nowrap}
.aruaru-lady-page .ox{overflow-x:auto}
.aruaru-lady-page .toc{display:flex;flex-wrap:wrap;gap:8px;margin:16px 0}
.aruaru-lady-page .toc a{display:inline-block;padding:6px 14px;border-radius:20px;background:rgba(190,24,93,.18);border:1px solid #be185d;color:#fda4af;font-size:15px;font-weight:700}
.aruaru-lady-page .toc a:hover{background:rgba(190,24,93,.35)}
.aruaru-lady-page .cron-box{background:#0c1228;border:1px solid #334155;border-radius:12px;padding:16px 18px;margin-top:32px;font-size:15px}
.aruaru-lady-page .cron-box code{background:#0a1628;padding:2px 8px;border-radius:4px;color:#a5f3fc;font-size:15px}
.aruaru-lady-page footer{text-align:center;padding:32px 16px;color:#64748b;font-size:15px;border-top:1px solid #1e293b;margin-top:48px}
@media(max-width:640px){.aruaru-lady-page .hero{padding:28px 14px}.aruaru-lady-page .card{padding:16px}}
</style>"#;

/// PHP側`aruaru-lady/index.php`(2916行)が実際に表示している内容を移植する。
/// 汎用JSONダンプ(`render_value_generic`)ではPHP版と全く別のページに
/// なってしまうため(2026-07-19監査、`/rakuten-mobile`と同じ問題)、この
/// 関数はPHP版のセクション構成・見出し・静的マーケティング文言(体験入店の
/// エリア一覧、政策提案カード等)を1対1に近い形で再現しつつ、データ部分
/// (キャバクラ/熟女キャバ/TVチャットレディ グループ・通常の各TOP50表)は
/// 既存の`fetch_cache`アーキテクチャ(HTTP経由で`aruaru-lady/*-cache.json`
/// 取得)をそのまま使う。CSSも実PHP版の`<style>`ブロックから移植済み
/// (`ARUARU_LADY_STYLE`、ユーザー指示によりスコープ拡大: 2026-07-19)。
async fn render_aruaru_lady_body() -> String {
    let caba = fetch_cache("aruaru-lady/aruaru-caba-ranking-cache.json").await;
    let jukujo = fetch_cache("aruaru-lady/aruaru-jukujo-caba-ranking-cache.json").await;
    let tv_group = fetch_cache("aruaru-lady/aruaru-tvchat-group-ranking-cache.json").await;
    let tv_normal = fetch_cache("aruaru-lady/aruaru-tvchat-normal-ranking-cache.json").await;

    let empty = Value::Null;
    let caba = caba.as_ref().unwrap_or(&empty);
    let jukujo = jukujo.as_ref().unwrap_or(&empty);
    let tv_group = tv_group.as_ref().unwrap_or(&empty);
    let tv_normal = tv_normal.as_ref().unwrap_or(&empty);

    let empty_rows: Vec<Value> = Vec::new();
    let caba_tokyo23: &[Value] = caba.get("tokyo_23").and_then(|v| v.get("rows")).and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let caba_tama: &[Value] = caba.get("tokyo_tama").and_then(|v| v.get("rows")).and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let caba_national: &[Value] = caba.get("national").and_then(|v| v.get("rows")).and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let caba_disclaimer = get_disp(caba, "disclaimer");
    let caba_updated = get_disp(caba, "updated_at");

    let jukujo_rows: &[Value] = jukujo.get("rows").and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let jukujo_disclaimer = get_disp(jukujo, "disclaimer");
    let jukujo_updated = get_disp(jukujo, "updated_at");

    let tvg_rows: &[Value] = tv_group.get("rows").and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let tvg_disclaimer = get_disp(tv_group, "disclaimer");
    let tvg_updated = get_disp(tv_group, "updated_at");

    let tvn_rows: &[Value] = tv_normal.get("rows").and_then(|v| v.as_array()).unwrap_or(&empty_rows);
    let tvn_disclaimer = get_disp(tv_normal, "disclaimer");
    let tvn_updated = get_disp(tv_normal, "updated_at");

    let caba_cols: &[(&str, &str)] = &[("area", "エリア"), ("title", "エリア名称・特徴"), ("band", "時給目安帯"), ("pickup", "補足")];
    let jukujo_cols: &[(&str, &str)] = &[("area", "エリア"), ("title", "エリア名称・特徴"), ("lux_band", "高級帯目安"), ("pickup", "補足")];
    let tv_cols: &[(&str, &str)] = &[("plat", "プラットフォーム"), ("title", "サイト/エリア・特徴"), ("band", "時給目安帯"), ("pickup", "補足")];

    let caba_tokyo23_table = render_rank_table(caba_tokyo23, "#fda4af", caba_cols);
    let caba_tama_table = render_rank_table(caba_tama, "#fda4af", caba_cols);
    let caba_national_table = render_rank_table(caba_national, "#fda4af", caba_cols);
    let jukujo_table = render_rank_table(jukujo_rows, "#fb7185", jukujo_cols);
    let tvg_table = render_rank_table(tvg_rows, "#fbbf24", tv_cols);
    let tvn_table = render_rank_table(tvn_rows, "#34d399", tv_cols);

    let tv_chat_nonadult_url = google_search_url("TVチャットレディ　ノンアダルト");
    let we2plus_url = google_search_url("楽天モバイル お勧め 1円 高性能CPU 富士通製 スマホ we2 plus");
    let link_android_url = google_search_url("楽天モバイル スマホアプリの名前は 楽天リンク Android版");
    let link_iphone_url = google_search_url("楽天モバイル スマホアプリの名前は 楽天リンク iPhone版");
    let jobs_ja_url = google_search_url("女性向け 求人情報");
    let jobs_en_url = google_search_url("Job postings for women in Japan");

    // 体験入店ゾーン一覧(PHP版`aruaru_taiken_zone_definitions()`、
    // 990〜980行目付近の13ゾーンをタイトル・紹介文のみ移植した簡略版。
    // PHP版は各ゾーン内をさらに市区町村単位のカードへ展開するが、今回は
    // コンテンツの一致(ゾーン名・紹介文・検索導線)を優先し、市区町村
    // カードの完全複製は省略した)。
    let taiken_zones: &[(&str, &str)] = &[
        ("東京都・23区内", "港区・新宿・渋谷・銀座ほか、区ごとの体験入店・体験勤務の募集例を検索する窓口です。"),
        ("東京都・23区外（多摩地域ほか）", "立川・八王子・町田・吉祥寺など、都心外の多摩・西東京エリアです。"),
        ("首都圏（神奈川・千葉・埼玉）主要都市", "横浜・川崎・千葉・さいたま・川口など、東京23区外の首都圏中核都市です。"),
        ("北海道", "札幌すすきのを中心に、旭川・函館など道内主要・区外エリアです。"),
        ("東北", "仙台・盛岡・青森など東北各県の主要歓楽街・駅前エリアです。"),
        ("関東（茨城・栃木・群馬・山梨）", "北関東・山梨の県都・歓楽街エリアです。"),
        ("甲信越・北陸（長野・新潟ほか）", "長野・新潟・金沢・富山など、中部山地と日本海側の主要エリアです。"),
        ("東海（愛知・静岡・岐阜・三重）", "名古屋・浜松・静岡・岐阜・四日市など東海地方の主要・区外都市です。"),
        ("関西（大阪・京都・神戸・奈良ほか）", "ミナミ・梅田・祇園・三宮など関西の中核と、堺・枚方など府県内区外です。"),
        ("中国地方", "広島・岡山・山口など中国地方の主要都市です。"),
        ("四国", "高松・松山・高知・徳島など四国4県の主要エリアです。"),
        ("九州（福岡・熊本・鹿児島ほか）", "中洲・熊本・天文館など九州各県の主要歓楽街・駅前です。"),
        ("沖縄", "那覇を中心に、沖縄本島の主要エリアです（離島・条例は各店で要確認）。"),
    ];
    let taiken_list: String = taiken_zones
        .iter()
        .map(|(title, intro)| {
            let url = google_search_url(&format!("{title} キャバクラ 熟女キャバ TVチャットレディ 体験入店"));
            format!(
                r#"<li style="margin:0 0 10px;"><a href="{url}" target="_blank" rel="noopener noreferrer" style="color:#bae6fd;font-weight:bold;">📍 {t}</a><br><span style="opacity:.8;font-size:14px;color:#cbd5e1;">{i}</span></li>"#,
                t = html_escape(title),
                i = html_escape(intro)
            )
        })
        .collect();

    format!(
        r##"{ARUARU_LADY_STYLE}
<div class="aruaru-lady-page">
<div class="hero">
  <h1>💃 女性向けお仕事情報</h1>
  <p>キャバレー・キャバクラ・熟女キャバ・TVチャットレディの求人・体験入店・時給目安など。各リンクは Google 検索窓口です。必ず各店の公式サイト・求人票・法令をご確認ください。</p>
</div>
<div class="wrap">

<div class="notice">
  <span>🔗</span>
  <span>IT技術・プログラミング・英会話など女性向け以外の情報は</span>
  <a href="/aruaru">audiocafe.tokyo/aruaru</a>
  <span>をご覧ください。</span>
</div>

<div class="toc">
  <a href="#lady-rakuten-mobile-corner">📶 楽天モバイル</a>
  <a href="#lady-jobs-corner">💼 女性向け求人</a>
  <a href="#tv-corner">📱 TVチャットレディ</a>
  <a href="#taiken-corner">🗺️ 体験入店</a>
  <a href="#caba-corner">🚗 キャバクラ時給TOP50</a>
  <a href="#jukujo-corner">✨ 熟女キャバTOP50</a>
</div>

<div class="card" id="lady-rakuten-mobile-corner">
<h2>📶 楽天モバイル</h2>
<p>スマホの乗り換えなら楽天モバイル。プラン・国際通話・プラチナバンドの最新情報は <a href="/rakuten-mobile">/rakuten-mobile</a> をご覧ください。</p>
</div>

<div class="card" id="lady-jobs-corner">
<h2>💼 女性向け・外資系 求人情報</h2>
<p style="color:#cbd5e1;font-size:14px;opacity:.92;">各リンクは Google 検索または転職サイトへの入口です。応募前に必ず各社の公式求人票・条件をご確認ください。</p>
<ul style="font-size:15px;line-height:2.1;padding-left:1.2rem;">
<li><a href="{jobs_ja_url}" target="_blank" rel="noopener noreferrer">🔍 女性向け 求人情報（Google検索）</a></li>
<li><a href="{jobs_en_url}" target="_blank" rel="noopener noreferrer">🔍 Job postings for women in Japan（Google Search）</a></li>
<li><a href="https://www.daijob.com/" target="_blank" rel="noopener noreferrer">🌐 Daijob.com　日本語</a></li>
<li><a href="https://www.daijob.com/en/" target="_blank" rel="noopener noreferrer">🌐 Daijob.com　ENGLISH</a></li>
</ul>
</div>

<div class="card" id="tv-corner">
<h2>📱 TVチャットレディ（ノンアダルト・在宅／駅前体験）</h2>
<p style="opacity:.9;font-size:15px;color:#e0f2fe;">日本全国の駅前には体験できるお店がある場合もあります。事前に検索し、各店の公式サイト（HP）で募集要項・年齢制限・機材・契約内容をご確認ください。自宅や病院で入院中でも、畳一畳分のスペースとスマートフォンがあれば始められる例もあります。</p>
<ul style="font-size:15px;line-height:2.1;padding-left:1.2rem;color:#cbd5e1;">
<li><a href="{tv_chat_nonadult_url}" target="_blank" rel="noopener noreferrer" style="color:#38bdf8;font-weight:bold;">🔍 Google検索：TVチャットレディ　ノンアダルト</a></li>
<li><a href="https://atgroup.jp/" target="_blank" rel="noopener noreferrer" style="color:#7dd3fc;font-weight:bold;">📱 ノンアダルト チャットレディ — atgroup.jp</a></li>
<li><a href="{we2plus_url}" target="_blank" rel="noopener noreferrer">🔍 楽天モバイル お勧め 富士通製 We2 Plus（1円・高性能CPU）</a></li>
<li><a href="{link_android_url}" target="_blank" rel="noopener noreferrer">🔍 楽天リンク Android版</a></li>
<li><a href="{link_iphone_url}" target="_blank" rel="noopener noreferrer">🔍 楽天リンク iPhone版</a></li>
</ul>
<p style="opacity:.88;font-size:15px;color:#cbd5e1;">楽天リンクアプリ経由の通話で電話放題プランなどを利用できる場合があります。<strong style="color:#fef08a;">TVチャットレディ</strong>は、まず<strong>スマートフォン</strong>から始めやすく、できれば<strong>WEBカメラ付きのオンラインPC環境</strong>もあると画質・安定性の面でお勧めです（各求人・各店の指定機材を必ずご確認ください）。</p>
</div>

<div class="card" id="taiken-corner">
<h2>🗺️ 体験入店・体験可能店（全国エリア別）</h2>
<p style="opacity:.88;font-size:15px;color:#c7d2fe;">体験入店・体験勤務の可否・年齢・契約・風俗営業適正化法等は店舗・自治体・求人媒体ごとに異なります。下記は各エリア名での Google 検索窓口のみで、特定店の推奨・保証ではありません。応募前に公式HP・求人票で必ずご確認ください。</p>
<div style="margin-bottom:14px;">
  <span style="color:#7dd3fc;font-weight:bold;margin-right:12px;">TVチャットレディ</span>
  <span style="color:#fb7185;font-weight:bold;margin-right:12px;">熟女キャバ</span>
  <span style="color:#fda4af;font-weight:bold;">キャバレー・キャバクラ</span>
</div>
<ul style="list-style:none;padding:0;">{taiken_list}</ul>
</div>

<div class="card" id="tvchat-group-corner">
<h2>📱 TVチャットレディ【グループチャット（パーティーチャット）版】 全国・高額時給目安ランキング TOP50</h2>
<div style="display:flex;align-items:center;gap:12px;background:linear-gradient(135deg,#7c1d4e 0%,#3b0764 100%);border:2px solid #fbbf24;border-radius:12px;padding:14px 18px;margin-bottom:14px;">
  <div style="flex-shrink:0;background:#fbbf24;color:#1c0a14;font-weight:900;font-size:18px;border-radius:50%;width:52px;height:52px;display:flex;align-items:center;justify-content:center;">No.1</div>
  <div>
    <div style="color:#fbbf24;font-weight:bold;">🏆 高額時給 No.1 — グループチャット（パーティーチャット）</div>
    <div style="color:#fef3c7;font-size:15px;">複数人同時参加で収入が大幅アップ。人数が多い場合、<strong style="color:#fbbf24;">時給 36,000円〜177,000円</strong> の例もあります。</div>
    <div style="margin-top:8px;"><a href="https://atgroup.jp/money" target="_blank" rel="noopener noreferrer" style="display:inline-block;background:#fbbf24;color:#1c0a14;font-weight:bold;padding:7px 20px;border-radius:8px;">💰 ATグループ 高額報酬ページを見る →</a></div>
  </div>
</div>
<p style="opacity:.88;font-size:15px;color:#fde68a;">{tvg_disclaimer}</p>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新（キャッシュ・毎日自動）: {tvg_updated} — 「高額時給順」は報酬率・同時接続効率の目安インデックスによる並びで、特定サイト/店舗の格付けではありません。</p>
{tvg_table}
</div>

<div class="card" id="tvchat-normal-corner">
<h2>📲 TVチャットレディ【通常版／1対1】 全国・高額時給目安ランキング TOP50</h2>
<p style="opacity:.88;font-size:15px;color:#fde68a;">{tvn_disclaimer}</p>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新（キャッシュ・毎日自動）: {tvn_updated} — 「高額時給順」は報酬率・稼働効率の目安インデックスによる並びで、特定サイト/店舗の格付けではありません。</p>
{tvn_table}
</div>

<div class="card" id="caba-corner">
<h2>🚗 キャバクラ・キャバレー 高額時給目安ランキング TOP50</h2>
<div style="margin-bottom:16px;padding:12px 14px;background:rgba(190,24,93,.12);border:1px solid rgba(190,24,93,.35);border-radius:10px;">
  <p style="font-size:15px;color:#fcd7e0;font-weight:700;">▶ 近年、若い男性受け・おじ様向けにも需要ニーズが増加中のスタイル — YouTube で検索：</p>
  <a href="https://www.youtube.com/results?search_query=%E3%82%B3%E3%82%B9%E3%83%97%E3%83%AC+%E3%83%9F%E3%83%8B%E3%82%B9%E3%82%AB+%E5%AD%A6%E5%9C%92+%E3%82%AD%E3%83%A3%E3%83%90" target="_blank" rel="noopener noreferrer" style="display:inline-flex;align-items:center;gap:6px;padding:7px 14px;border-radius:20px;background:#be185d;color:#fff;font-weight:800;">▶ コスプレ　ミニスカ　学園　キャバ</a>
</div>
<p style="opacity:.82;font-size:15px;color:#fecdd3;">いずれも Google 検索窓口へのリンクです。時給は目安であり、各店の求人票を必ずご確認ください。</p>
<h3 id="aruaru-caba-tokyo23">🚗 高額時給目安ランキング TOP50（東京23区内・キャバレー／キャバクラ・検索窓口）</h3>
<p style="opacity:.88;font-size:15px;color:#fde68a;">{caba_disclaimer}</p>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新: {caba_updated} — TTL 約7日</p>
{caba_tokyo23_table}
<h3 id="aruaru-caba-tokyo-tama">🚗 高額時給目安ランキング TOP50（東京23区外・多摩／八王子・立川ほか・検索窓口）</h3>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新: {caba_updated} — TTL 約7日</p>
{caba_tama_table}
<h3 id="aruaru-caba-national">🚗 高額時給目安ランキング TOP50（大阪〜北海道・沖縄・日本全国・検索窓口）</h3>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新: {caba_updated} — TTL 約7日</p>
{caba_national_table}
</div>

<div class="card" id="jukujo-corner">
<h2>✨ 熟女キャバレー・熟女キャバクラ 全国・高級帯目安 TOP50</h2>
<p style="opacity:.88;font-size:15px;color:#fecdd3;">{jukujo_disclaimer}</p>
<p style="opacity:.65;font-size:15px;color:#cbd5e1;">📅 リスト更新（キャッシュ）: {jukujo_updated} — 「高級順」はエリアの目安インデックスによる並びで、特定店の格付けではありません。</p>
{jukujo_table}
</div>

<div class="cron-box">
  📡 <strong>★ Cron 自動更新設定（毎日）：</strong> キャバクラ・熟女キャバ・TVチャットレディ（グループ/通常）の各ランキングキャッシュは毎朝自動更新されます。
</div>

<div class="notice" style="margin-top:32px;">
  <span>🔗</span>
  <span>IT技術・プログラミング・英会話など女性向け以外の情報は</span>
  <a href="/aruaru">audiocafe.tokyo/aruaru</a>
  <span>をご覧ください。</span>
</div>

</div>

<div class="card" style="border-color:#4338ca;margin-top:32px;" id="policy-sougeishaxi">
  <h2 style="color:#a5b4fc;">🚗 関連政策提案：無料送迎車の半公共タクシー化</h2>
  <h3 style="color:#c7d2fe;">概念：半公共送迎タクシー制度</h3>
  <p style="font-size:15px;color:#e2e8f0;">キャバクラ等の無料送迎車を、空席があれば一般市民も乗れる<strong style="color:#c7d2fe;">「相乗り型半公共交通」</strong>として機能させる構想です。病院・スーパーへの買い物など日常的な送迎にも活用でき、交通空白地帯の補完手段となり得ます。運転手には店からの給与に加え、<strong style="color:#c7d2fe;">日本全国の市区町村から半公務員手当</strong>が支給される仕組みとすることで、安定した収入と社会的役割を両立できる可能性があります。</p>
  <h3 style="color:#86efac;font-size:16px;">✅ メリット</h3>
  <ul style="font-size:15px;line-height:2.1;padding-left:1.4rem;color:#d1fae5;">
    <li>深夜・郊外など交通空白地帯をカバー</li>
    <li>運転手の収入が安定（店＋自治体手当の二重収入）</li>
    <li>病院・買い物難民の高齢者救済</li>
    <li>車両の遊休時間を社会還元</li>
  </ul>
  <h3 style="color:#fca5a5;font-size:16px;">⚠️ 課題と論点</h3>
  <ul style="font-size:15px;line-height:2.1;padding-left:1.4rem;color:#fecdd3;">
    <li>道路運送法との整合性（白タク規制）</li>
    <li>自治体ごとの財源・予算手当</li>
    <li>乗客の安全管理・身元確認</li>
    <li>優先度のルール（店の業務客 vs 一般市民）</li>
  </ul>
  <h3 style="color:#a5b4fc;font-size:16px;">📋 近い事例</h3>
  <ul style="font-size:15px;line-height:2.1;padding-left:1.4rem;color:#c7d2fe;">
    <li><strong>デマンド型乗合タクシー</strong> — 過疎地で既に実施中。予約に応じて乗合運行する公共交通の仕組み。</li>
    <li><strong>ライドシェア</strong> — 2024年から一部解禁。自家用車を使った有償旅客運送が条件付きで認められ始めた。</li>
    <li><strong>スクールバスの地域開放</strong> — 登下校時間外に地域住民が利用できるよう開放する取り組み。</li>
  </ul>
  <p style="font-size:15px;color:#94a3b8;">実現には道路運送法（白タク規制）の整備・自治体ごとの条例・財源確保・安全管理ルールの策定が必要です。既存のデマンド交通やライドシェア制度との連携・法改正が検討課題となります。</p>
  <hr style="border:none;border-top:1px solid rgba(148,163,184,.2);margin:20px 0;">
  <h3 style="color:#fcd34d;font-size:16px;">🍺 お客様送迎・昼飲み需要・販売品目に関する提案</h3>
  <p style="font-size:15px;color:#e2e8f0;">キャバレー・キャバクラでは従業員向けの自動車での<strong style="color:#fcd34d;">無料送迎サービス</strong>がある所が多い様ですので、<strong style="color:#fcd34d;">お客様向けにも無料の送迎サービス</strong>があった方が親切で良いと思われます。またお店も、朝からや昼からでも楽しく飲んで気分転換したい方の<strong style="color:#fcd34d;">需要やニーズも増加中</strong>の様です。</p>
  <h3 style="color:#7dd3fc;font-size:15px;">🥤 飲料・販売品目の拡充提案</h3>
  <p style="font-size:15px;color:#e2e8f0;">キャバレー・キャバクラや、今後は<strong style="color:#7dd3fc;">駅のKIOSK・キヨスクや自動販売機</strong>でも、ウイスキーやビールの他に、<strong style="color:#7dd3fc;">ASAHI の青い缶の無添加のノンアルコールビール</strong>など販売や提供などの需要やニーズが増加中の様です。</p>
  <h3 style="color:#f9a8d4;font-size:15px;">💊 心臓ケア商品の優先販売提案</h3>
  <p style="font-size:15px;color:#e2e8f0;">駅のキオスクや自動販売機、キャバレー・キャバクラなどのお店でも、心臓の薬の<strong style="color:#f9a8d4;">「救心」</strong>や、心臓に良いサプリメントとして<strong style="color:#f9a8d4;">「コエンザイムQ10」</strong>などを<strong style="color:#f9a8d4;">優先的に販売</strong>して欲しいです。</p>
</div>

<div style="margin:12px 0 18px;padding:14px 18px;border-radius:12px;background:linear-gradient(135deg,rgba(255,122,69,.13),rgba(47,111,237,.08));border:1.5px solid #ff7a45;display:flex;align-items:center;flex-wrap:wrap;gap:10px;">
  <span style="font-size:18px;">🎲</span>
  <span style="color:#ff7a45;font-weight:700;font-size:15px;">「あるある」まとめ & 開発リポジトリ紹介はこちら →</span>
  <a href="{ARUARU_TOKYO_URL}" target="_blank" rel="noopener noreferrer" style="display:inline-block;padding:6px 16px;border-radius:8px;background:linear-gradient(135deg,#ff7a45,#2f6fed);color:#fff;font-weight:800;">📍 aruaru.tokyo</a>
</div>

<footer>
  <p>© audiocafe.tokyo / aruaru-lady — 掲載情報はGoogle検索窓口へのリンクです。各店・各媒体の公式情報を必ずご確認ください。</p>
  <p style="margin-top:6px;">年齢制限・法令・各都道府県の条例に必ず従ってください。</p>
</footer>
</div>
"##
    )
}

/// PHP版`aruaru/index.php`(8152行)が実際に外部リンク集として掲載している
/// 求人サイト19件(2395〜2452行目、`EXT_SITES`定数)。(表示名, タグ, 紹介文, リンク先)。
/// 「ITあんけん」だけは実URLが動的生成(`build_itanken_url`、free_wordクエリの
/// 複雑な組み立てロジック)のため、簡略化してGoogle検索窓口にフォールバックした。
const ARUARU_EXT_SITES: &[(&str, &str, &str, &str)] = &[
    ("レバテックフリーランス", "フリーランス", "案件数・単価ともに国内最大級のITフリーランス向けエージェント。高単価・長期案件が豊富。", "https://freelance.levtech.jp/project/"),
    ("フリーランススタート", "フリーランス", "50万件以上の案件を集約した最大級フリーランス案件データベース。キーワードで複数スキルを組み合わせて検索可能。", "https://freelance-start.com/jobs?keyword="),
    ("ITあんけん", "フリーランス", "50万件超の案件データベース。言語とフレームワークを組み合わせて検索可能。", "https://www.google.com/search?q=ITあんけん"),
    ("Wantedly", "スタートアップ", "「やりたいこと」でつながる採用サービス。スタートアップ・ベンチャーの求人が豊富。", "https://www.wantedly.com/projects"),
    ("ITプロパートナーズ", "副業・フリーランス", "週2〜3日から参画できる副業・フリーランス案件に特化。スタートアップ系が豊富。", "https://itpropartners.com/job?free_word="),
    ("リクルートエージェント", "正社員求人", "リクルートの正社員求人サイト。言語キーワード(例: Python)で求人検索。", "https://www.r-agent.com/job_search/"),
    ("ハイパフォコンサル", "コンサルフリーランス", "PM・PMO・戦略・SAP・IT・AI領域のフリーランスコンサル案件紹介。エンド直・高単価案件多数。", "https://www.high-performer.jp/consultant/projects/?onlyRecruiting=true"),
    ("geechs job", "フリーランス", "国内最大級のギークスジョブ。ITフリーランスの高単価・リモート案件が豊富。", "https://geechs-job.com/project/"),
    ("Midworks", "フリーランス", "フリーランスでも社会保険・各種保障が充実。正社員並みのサポートで安心して働ける。", "https://mid-works.com/projects/skills/"),
    ("クラウドテック", "フリーランス", "クラウドワークスが運営するITフリーランス向けエージェント。多様な職種・単価帯。", "https://tech.crowdworks.jp/job_offers/o/1?q="),
    ("Findy Freelance", "フリーランス", "GitHubスキルスコアで自動マッチング。エンジニア目線のフリーランス案件サービス。", "https://freelance.findy-code.io/works/languages/"),
    ("フリーランスハブ", "フリーランス", "複数のフリーランス案件サイトを横断検索できるアグリゲーター。効率よく案件を探せる。", "https://freelance-hub.jp/"),
    ("ココナラテック", "フリーランス", "ココナラが運営するITフリーランス向けエージェント。技術スキルに特化した案件を検索可能。", "https://tech.coconala.co.jp/"),
    ("Offers", "副業・複業", "副業・複業×開発案件のマッチング。スタートアップや成長企業の週1〜案件が充実。", "https://offers.jp/jobs/skills/"),
    ("Green", "正社員転職", "IT・Web・ゲーム業界特化の転職サービス。正社員でキャリアアップしたい方向け。", "https://www.green-japan.com/search/skill/"),
    ("Findy(転職)", "エンジニア転職", "スキルスコアでスカウトが届くエンジニア特化の転職サービス。高年収求人多数。", "https://findy-code.io/recommends/"),
    ("Indeed Japan", "総合求人", "国内最大級の求人検索エンジン。正社員・契約社員・フリーランスを幅広く検索可能。", "https://jp.indeed.com/jobs?q="),
    ("Daijob(日本語)", "バイリンガル・外資系", "日本最大級のバイリンガル・外資系・グローバル企業向け転職サイト(日本語版)。", "https://www.daijob.com/jobs/search_result?kw="),
    ("Daijob (English)", "Bilingual / Global", "Japan's largest bilingual / foreign-affiliated career site.", "https://www.daijob.com/en/jobs/search_result?kw="),
];

/// PHP版`aruaru_learning_categories()`(892〜1121行目)が持つ5カテゴリの
/// TOP50学習サービスのうち、各カテゴリの実データ(自動生成の穴埋め用
/// 「〇〇 おすすめ #51」等のパディング行は除く)を代表数件だけ抜粋移植した
/// もの(タイトル, [(名称,紹介文,料金)])。PHP版はカテゴリごとに80/50件まで
/// 機械的にパディングしているが、内容一致の本質(実在するカテゴリと代表的な
/// サービス名が分かること)には影響しないと判断し、正直に開示した上で
/// 簡略化した(aruaru-ladyの体験入店ゾーン簡略化と同じ方針)。
const ARUARU_LEARNING_CATEGORIES: &[(&str, &[(&str, &str, &str)])] = &[
    ("おすすめ学習塾 TOP50", &[
        ("河合塾(Kawaijuku)", "大学受験・理系強化・全国展開", "年間数十万円〜"),
        ("駿台予備学校", "難関大・医学部・理系特化", "年間数十万円〜"),
        ("早稲田アカデミー", "小中高・大学受験", "月額1〜3万円前後〜"),
        ("スタディサプリ進学", "動画＋進路・オンライン塾", "月額数千円〜"),
        ("トライ", "家庭教師・個別", "月額3〜6万円前後〜"),
        ("SAPIX", "中学受験", "年間数十万円〜"),
        ("ヒューマンアカデミー", "IT・Web・デザイン", "数十万円〜(講座による)"),
    ]),
    ("おすすめ家庭教師紹介サービス TOP50", &[
        ("家庭教師のトライ", "全国ネットワーク・オンライン可・講師紹介", "時間数・単価は要確認"),
        ("あすなろ家庭教師", "マンツーマン・定期・紹介", "地域・単価要確認"),
        ("マナリンク(オンライン家庭教師)", "マッチング型・オンライン中心", "講師単価は要確認"),
        ("スタディサプリ(個別オンライン指導)", "大手・オンラインマンツーマン事例あり", "プランにより要確認"),
    ]),
    ("おすすめPC教室 TOP50", &[
        ("パソコン教室ワード", "Office・基礎操作", "月額5千〜1.5万円〜"),
        ("キッズパソコン教室", "子ども向け", "月額5千〜1万円〜"),
        ("アビバ", "資格・Office", "コースによる"),
        ("ライズ", "PC・資格", "コースによる"),
    ]),
    ("おすすめプログラミング教室 TOP50", &[
        ("テックアカデミー", "Web・Ruby/Python", "月額〜数十万(プランによる)"),
        ("ドットインストール", "動画で手軽に", "月額980円台〜"),
        ("Progate", "初心者向け", "月額980円台〜"),
        ("Schoo", "ライブ授業", "月額2,178円〜"),
        ("paizaラーニング", "問題演習", "月額980円台〜"),
        ("AtCoder", "競プロ", "無料"),
        ("Qiita / Zenn", "技術記事", "無料"),
    ]),
    ("おすすめ学習タブレット TOP50", &[
        ("Smile Zemi", "小中学生", "月額〜"),
        ("Z会のタブレット", "中学受験", "月額〜"),
        ("進研ゼミ デジタル", "小中高", "月額〜"),
        ("スタディサプリ", "動画学習", "月額〜"),
    ]),
];

/// PHP版`aruaru/index.php`(8152行)の`<style>`ブロック(4222〜4380行目)
/// から、レイアウトの核となるダークテーマCSSを移植したもの
/// (`.aruaru-page`でスコープし、他ページの同名セレクタと衝突しないよう
/// にしている)。ロゴcanvasアニメーション・検索フォーム/チップ演出用JS・
/// Google翻訳ウィジェット用CSSは対象外(ユーザー指示によるスコープ:
/// 見た目の一致は静的レイアウト・配色が対象であり、JS演出は既存2ページ
/// (aruaru-lady・rakuten-mobile)と同じく対象外と判断)。
const ARUARU_STYLE: &str = r#"<style>
.aruaru-page{margin:-2rem -1rem;background:#0b1220;color:#fff;font-family:'Noto Sans JP','Hiragino Kaku Gothic ProN',Meiryo,system-ui,sans-serif;line-height:1.7}
.aruaru-page a{color:#7dd3fc;text-decoration:none}
.aruaru-page a:hover{text-decoration:underline}
.aruaru-page .wrap{max-width:1100px;margin:0 auto;padding:16px}
.aruaru-page .hero{padding:2.4rem 1rem 1.6rem;text-align:center}
.aruaru-page .hero h1{font-size:clamp(1.5rem,4.5vw,2.4rem);font-weight:900;line-height:1.3;background:linear-gradient(135deg,#fff 0%,#cfe9ff 55%,#a5b4fc 100%);-webkit-background-clip:text;background-clip:text;color:transparent}
.aruaru-page .hero p{color:#dde6f5;font-size:15px;max-width:44rem;margin:.75rem auto 0}
.aruaru-page .hero-stats{display:flex;flex-wrap:wrap;justify-content:center;gap:1.6rem;margin-top:1.4rem}
.aruaru-page .hero-stat-val{font-size:1.3rem;font-weight:900;display:block}
.aruaru-page .hero-stat-lbl{font-size:.85rem;color:#dde6f5}
.aruaru-page .card{background:linear-gradient(165deg,#111c33 0%,#16223d 100%);border:1px solid rgba(255,255,255,.08);border-radius:14px;padding:20px 22px;margin:24px 0}
.aruaru-page h2{color:#fff;font-size:clamp(18px,4vw,24px);margin-bottom:12px}
.aruaru-page h3{font-size:16px;margin:20px 0 8px}
.aruaru-page table{width:100%;border-collapse:collapse;font-size:15px}
.aruaru-page thead tr{background:#10233f;text-align:left}
.aruaru-page th{padding:8px 6px;white-space:nowrap}
.aruaru-page .ox{overflow-x:auto}
.aruaru-page .toc{display:flex;flex-wrap:wrap;gap:8px;margin:16px 0}
.aruaru-page .toc a{display:inline-block;padding:6px 14px;border-radius:20px;background:rgba(6,182,212,.12);border:1px solid rgba(6,182,212,.4);color:#7dd3fc;font-size:15px;font-weight:700}
.aruaru-page .toc a:hover{background:rgba(6,182,212,.25)}
.aruaru-page .ext-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:12px}
.aruaru-page .ext-card{display:block;background:#16223d;border:1px solid rgba(255,255,255,.08);border-radius:11px;padding:14px 16px}
.aruaru-page .ext-card:hover{border-color:rgba(255,255,255,.25)}
.aruaru-page .ext-card-name{font-weight:800;color:#fff}
.aruaru-page .ext-tag{font-size:.8rem;font-weight:800;padding:1px 7px;border-radius:5px;background:rgba(167,139,250,.16);color:#c4b5fd;margin-left:6px}
.aruaru-page .ext-desc{font-size:.86rem;color:#dde6f5;margin:.3rem 0}
.aruaru-page .notice{background:rgba(190,24,93,.1);border:1.5px solid #fda4af;border-radius:12px;padding:12px 16px;margin:18px 0;display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.aruaru-page footer{text-align:center;padding:32px 16px;color:#94a3b8;font-size:14px;border-top:1px solid rgba(255,255,255,.08);margin-top:40px}
@media(max-width:640px){.aruaru-page .card{padding:14px}}
</style>"#;

/// ランキング/一覧系のデータをPHP版と同じ列見出しの表として描画する
/// 汎用ヘルパー(`render_rank_table`の`aruaru`版、検索リンク列は付けない)。
/// `name`列に`url`フィールドがあれば自動的にリンク化する
/// (英会話ランキングのアプリ名リンク等、PHP版の挙動を再現)。
fn render_data_table(rows: &[Value], head_color: &str, cols: &[(&str, &str)]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::from(r#"<div class="ox"><table><thead><tr>"#);
    for (_, label) in cols {
        out.push_str(&format!(r#"<th style="color:{head_color};">{}</th>"#, html_escape(label)));
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str(r#"<tr style="border-bottom:1px solid rgba(255,255,255,.08);">"#);
        for (key, _) in cols {
            let cell = if *key == "name" {
                let name = get_disp(row, "name");
                match row.get("url").and_then(|v| v.as_str()) {
                    Some(url) if !url.is_empty() => format!(
                        r#"<a href="{}" target="_blank" rel="noopener noreferrer" style="color:{head_color};font-weight:bold;">{}</a>"#,
                        html_escape(url), html_escape(&name)
                    ),
                    _ => html_escape(&name),
                }
            } else {
                html_escape(&get_disp(row, key))
            };
            out.push_str(&format!(r#"<td style="padding:6px 8px;">{cell}</td>"#));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    out
}

/// PHP版`aruaru/index.php`(8152行)が実際に表示している内容を移植する。
/// 汎用JSONダンプ(`render_value_generic`)ではPHP版と全く別のページに
/// なってしまうため(2026-07-19、`/rakuten-mobile`・`/aruaru-lady`と同種の
/// 問題を監査で確認——旧`COMPOSITE_PAGES`の`aruaru`エントリはキャバクラ/
/// 熟女キャバランキングを列挙していたが、実際のPHP版はそれらを
/// `/aruaru-lady`へ移転済みで、`/aruaru`側ではもう表示していない)。
/// この関数はPHP版の実際のセクション構成
/// (ヒーロー→楽天モバイル導線→doda求人ピックアップ→外部求人サイト一覧→
/// サービス向上・販売提案→AI技術TOP80ランキング(言語/FW/DB)→
/// 学習サービスTOP50→英会話TOP50→aruaru-lady移転案内)を再現しつつ、
/// データ部分は既存の`fetch_cache`アーキテクチャ経由で取得する。
/// CSSは`ARUARU_STYLE`(PHP版`<style>`ブロックの核部分を移植、ユーザー
/// 指示によるスコープ拡大: 見た目もPHP版と一致させる)。
async fn render_aruaru_body() -> String {
    let doda = fetch_cache("aruaru/doda-jobs-cache.json").await;
    let tech = fetch_cache("ai-tech-ranking-cache.json").await;
    let eikaiwa = fetch_cache("aruaru-eikaiwa-ranking-cache.json").await;

    let empty = Value::Null;
    let doda = doda.as_ref().unwrap_or(&empty);
    let tech = tech.as_ref().unwrap_or(&empty);
    let eikaiwa = eikaiwa.as_ref().unwrap_or(&empty);

    let empty_rows: Vec<Value> = Vec::new();
    let doda_updated = get_disp(doda, "updated_human");

    // PHP版`doda_categories()`(`aruaru/index.php` 4754〜4763行目)と同じ
    // 実際のdoda検索結果URL(未経験可・転勤無しで絞り込み済み)をフォールバック
    // に設定(2026-07-19、ユーザー指摘により修正——従来はキャッシュに
    // `search`フィールドが無い場合、汎用のdoda.jpトップページに飛ぶだけで
    // 「IT・通信業界」「広告・マーケティング業界」で絞り込まれていなかった)。
    let doda_categories: String = [
        (
            "it",
            "💼 IT・通信業界(未経験可／転勤無し)",
            "https://doda.jp/DodaFront/View/JobSearchList.action?ss=1&op=17,70,71,27,24&pic=1&ds=0&ind=01L&tp=1&bf=1&mpsc_sid=10&oldestDayWdtno=0&leftPanelType=1",
        ),
        (
            "ad",
            "📢 広告・マーケティング業界(未経験可／転勤無し)",
            "https://doda.jp/DodaFront/View/JobSearchList.action?ss=1&op=17,70,71,27,24&pic=1&ds=0&ci=131041&ind=1101S,1108S&tp=1&bf=1&mpsc_sid=10&oldestDayWdtno=0&leftPanelType=1",
        ),
    ]
        .iter()
        .map(|(key, fallback_label, fallback_search)| {
            let cat = doda.get("categories").and_then(|c| c.get(key));
            let label = cat.and_then(|c| c.get("label")).and_then(|v| v.as_str()).unwrap_or(fallback_label).to_string();
            let search = cat.and_then(|c| c.get("search")).and_then(|v| v.as_str()).unwrap_or(fallback_search).to_string();
            let items: &[Value] = cat.and_then(|c| c.get("items")).and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&empty_rows);
            let item_list: String = items
                .iter()
                .take(12)
                .map(|it| {
                    let title = get_disp(it, "title");
                    let url = get_disp(it, "url");
                    format!(
                        r#"<li style="border-bottom:1px solid rgba(148,163,184,.18);"><a href="{}" target="_blank" rel="noopener noreferrer nofollow" style="display:block;padding:6px 2px;font-size:14px;">{}</a></li>"#,
                        html_escape(&url), html_escape(&title)
                    )
                })
                .collect();
            format!(
                r#"<div style="padding:16px 18px;border-radius:14px;background:rgba(2,6,23,.85);border:1px solid rgba(59,130,246,.4);">
<h3 style="margin:0 0 8px;font-size:15px;">{}</h3>
<ul style="list-style:none;margin:0 0 10px;padding:0;">{}</ul>
<a href="{}" target="_blank" rel="noopener noreferrer nofollow" style="display:block;text-align:center;background:#d12d36;color:#fff;border-radius:8px;padding:9px 14px;font-size:13px;text-decoration:none;">doda で最新の求人一覧を見る ▶</a>
</div>"#,
                html_escape(&label), item_list, html_escape(&search)
            )
        })
        .collect();

    let ext_cards: String = ARUARU_EXT_SITES
        .iter()
        .map(|(name, tag, desc, url)| {
            format!(
                r#"<a href="{url}" target="_blank" rel="noopener noreferrer" class="ext-card">
<div><span class="ext-card-name">{name}</span><span class="ext-tag">{tag}</span></div>
<div class="ext-desc">{desc}</div>
<div style="font-size:.86rem;color:#7dd3fc;font-weight:700;">このサイトへ →</div>
</a>"#,
                name = html_escape(name), tag = html_escape(tag), desc = html_escape(desc), url = html_escape(url)
            )
        })
        .collect();

    let lang_rows: &[Value] = tech.get("languages").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&empty_rows);
    let fw_rows: &[Value] = tech.get("frameworks").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&empty_rows);
    let db_rows: &[Value] = tech.get("databases").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&empty_rows);
    let tech_updated = get_disp(tech, "updated_at");

    let lang_cols: &[(&str, &str)] = &[
        ("rank", "順位"), ("name", "言語"), ("team_dev", "チーム開発"), ("maintenance", "保守性"),
        ("beginner", "初心者向け"), ("speed", "速度"), ("memory", "必要メモリ容量"), ("dev_scale", "開発規模"),
        ("traits", "特徴"), ("oss_note", "オープンソース"), ("async_support", "非同期対応"), ("ai_comment", "AI分析コメント"),
    ];
    let fw_cols: &[(&str, &str)] = &[
        ("rank", "順位"), ("name", "Framework"), ("team_dev", "チーム開発"), ("maintenance", "保守性"),
        ("beginner", "初心者向け"), ("speed", "速度"), ("memory", "必要メモリ容量"), ("large_scale", "大規模開発"),
        ("ai_comment", "AI分析コメント"),
    ];
    let db_cols: &[(&str, &str)] = &[
        ("rank", "順位"), ("name", "DATABASE"), ("speed", "処理速度"), ("scale", "スケール"),
        ("distributed", "分散対応"), ("memory", "必要メモリ容量"), ("ai_comment", "AI分析コメント"),
    ];
    let lang_table = render_data_table(lang_rows, "#00ffff", lang_cols);
    let fw_table = render_data_table(fw_rows, "#ffaa00", fw_cols);
    let db_table = render_data_table(db_rows, "#ff66cc", db_cols);

    // 2026-07-19、ユーザー指示により追加: 言語ランキングの下にVersionlessAPI、
    // フレームワークランキングの下にWunderGraph Cosmoへの日本語/英語
    // それぞれのGoogle検索結果リンクを追記(`hl=ja`/`hl=en`で検索UI言語を
    // 切り替え、`google_search_url`と同じクエリ組み立て方針)。
    let versionless_api_ja_url = html_escape(&format!("{}&hl=ja", google_search_url("VersionlessAPI")));
    let versionless_api_en_url = html_escape(&format!("{}&hl=en", google_search_url("VersionlessAPI")));
    let wundergraph_cosmo_ja_url = html_escape(&format!("{}&hl=ja", google_search_url("WunderGraph Cosmo")));
    let wundergraph_cosmo_en_url = html_escape(&format!("{}&hl=en", google_search_url("WunderGraph Cosmo")));

    let learning_sections: String = ARUARU_LEARNING_CATEGORIES
        .iter()
        .map(|(title, rows)| {
            let items: String = rows
                .iter()
                .map(|(name, feat, price)| {
                    format!(
                        r#"<li style="border-bottom:1px solid rgba(255,255,255,.08);padding:6px 0;"><strong>{}</strong> — {} <span style="color:#fde68a;">{}</span></li>"#,
                        html_escape(name), html_escape(feat), html_escape(price)
                    )
                })
                .collect();
            format!(
                r#"<h3>{}</h3><ul style="list-style:none;margin:0;padding:0;font-size:15px;">{}</ul>"#,
                html_escape(title), items
            )
        })
        .collect();

    let eikaiwa_rows: &[Value] = eikaiwa.get("rows").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&empty_rows);
    let eikaiwa_updated = get_disp(eikaiwa, "updated_at");
    let eikaiwa_cols: &[(&str, &str)] = &[
        ("rank", "順位"), ("name", "アプリ・サービス名"), ("platform", "対応端末"),
        ("style", "学習スタイル"), ("level", "レベル"), ("price", "料金目安"), ("note", "ポイント・特徴"),
    ];
    let eikaiwa_table = render_data_table(eikaiwa_rows, "#34d399", eikaiwa_cols);

    let it_kenshu_url = google_search_url("未経験 無料IT研修 無料 転職エージェント サービス");
    let daiku_url = google_search_url("未経験から大工 造作大工 型枠大工 木造建築大工");
    let kenchiku_kanri_url = google_search_url("未経験・無資格から 一級建築士 木造建築士 管理建築士 1級建築施工管理技士 を目指せる求人");

    format!(
        r##"{ARUARU_STYLE}
<div class="aruaru-page">
<div class="hero">
  <h1>スキルと希望条件から<br>あなたにぴったりの案件が見つかる。</h1>
  <p>言語・フレームワーク・月額・勤務地で絞り込み。マッチした案件の外部サイトへ直接応募 ＋ 似た求人が見つかる外部サービスもご紹介。</p>
  <div class="hero-stats">
    <div><span class="hero-stat-val">32</span><span class="hero-stat-lbl">掲載案件</span></div>
    <div><span class="hero-stat-val">19</span><span class="hero-stat-lbl">外部求人サイト</span></div>
    <div><span class="hero-stat-val">80%</span><span class="hero-stat-lbl">リモート対応</span></div>
    <div><span class="hero-stat-val">¥60〜130万</span><span class="hero-stat-lbl">月額レンジ</span></div>
  </div>
</div>
<div class="wrap">

<div class="toc">
  <a href="#aruaru-rakuten-mobile-corner">📶 楽天モバイル</a>
  <a href="#doda-jobs">💼 doda求人ピックアップ</a>
  <a href="#ext">🌐 外部求人サイト</a>
  <a href="#policy-service">💡 サービス向上・販売提案</a>
  <a href="#aruaru-top80-tech">🚀 技術ランキングTOP80</a>
  <a href="#aruaru-learning">📚 学習サービスTOP50</a>
  <a href="#aruaru-eikaiwa-top50">🌏 英会話TOP50</a>
</div>

<div class="card" id="aruaru-rakuten-mobile-corner">
<h2>📶 楽天モバイル</h2>
<p>スマホの乗り換えなら楽天モバイル。プラン・国際通話・プラチナバンドの最新情報は <a href="/rakuten-mobile">/rakuten-mobile</a> をご覧ください。</p>
</div>

<div class="card" id="doda-jobs">
<h2>💼 転職求人ピックアップ（doda）</h2>
<p style="color:#94a3b8;font-size:13px;">未経験可・転勤無しの条件で毎日自動更新{doda_updated_note}</p>
<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:14px;">{doda_categories}</div>
</div>

<div class="card" id="ext">
<h2>🌐 外部求人サイト（{ext_count}件）</h2>
<p style="color:#dde6f5;font-size:.86rem;">案件検索・応募は各サイトへ直接遷移します。</p>
<div class="ext-grid">{ext_cards}</div>
</div>

<div class="card" id="policy-service">
<h2>💡 サービス向上・販売提案</h2>
<h3>🚗 お客様向け無料送迎サービスの導入を</h3>
<p style="font-size:15px;color:#e2e8f0;">キャバレー・キャバクラでは従業員向けの自動車での無料送迎サービスがある所が多い様ですので、<strong>お客様向けにも無料の送迎サービス</strong>があった方が親切で良いと思われます。</p>
<h3>☀️ 朝・昼からのOPENのお店を増やして欲しい</h3>
<p style="font-size:15px;color:#e2e8f0;">お店も、朝からや昼からでも楽しく飲んで気分転換したい方の需要やニーズも増加中の様ですので、<strong>開店時間を朝からや昼からもOPENのお店</strong>を増やして欲しいです。</p>
<h3>🍺 ノンアルコール・無添加ビールの販売拡大を</h3>
<p style="font-size:15px;color:#e2e8f0;">キャバレー・キャバクラや今後は駅のKIOSK・キヨスクや自動販売機でも、ウイスキーやビールの他に、<strong>ASAHIの青い缶の無添加のノンアルコールビール</strong>など販売や提供などの需要やニーズが増加中の様です。</p>
<h3>💊 心臓に良い薬・サプリメントの優先販売を</h3>
<p style="font-size:15px;color:#e2e8f0;">駅のキオスクや自動販売機やキャバレー・キャバクラなどのお店でも、心臓の薬の<strong>「救心」</strong>や心臓に良いサプリメントとして<strong>「コエンザイムQ10」</strong>などを優先的に販売して欲しいです。</p>
<p style="font-size:13px;color:#94a3b8;">※ 上記はサービス向上・販売促進に関する提案です。各種法令・条例・販売規制を必ずご確認ください。</p>
</div>

<div class="card" id="aruaru-top80-tech">
<h2 style="color:#00ffff;">🚀 人気TOP200 技術ランキング（AI自動分析）</h2>
<p style="opacity:.7;font-size:15px;">🕐 最終更新: {tech_updated}</p>
<h3 style="color:#00ffaa;">💻 人気プログラミング言語 TOP200</h3>
{lang_table}
<p style="font-size:14px;margin:10px 0 0;">
<a href="{versionless_api_ja_url}" target="_blank" rel="noopener noreferrer">VersionlessAPI（日本語で検索）</a>
　|
<a href="{versionless_api_en_url}" target="_blank" rel="noopener noreferrer">VersionlessAPI (search in English)</a>
</p>
<h3 style="color:#ffaa00;">⚡ 人気フレームワーク TOP200</h3>
{fw_table}
<p style="font-size:14px;margin:10px 0 0;">WunderGraph Cosmoと言うRestAPI不要などの特徴を持ちます。
<a href="{wundergraph_cosmo_ja_url}" target="_blank" rel="noopener noreferrer">WunderGraph Cosmo（日本語で検索）</a>
　|
<a href="{wundergraph_cosmo_en_url}" target="_blank" rel="noopener noreferrer">WunderGraph Cosmo (search in English)</a>
</p>
<h3 style="color:#ff66cc;">🗄 DATABASE ランキング TOP200</h3>
{db_table}
</div>

<div class="card" id="aruaru-learning">
<h2 style="color:#a5f3fc;">📚 おすすめ学習サービス TOP50（日本語・英語）</h2>
<p style="opacity:.75;font-size:15px;">学習塾・家庭教師紹介サービス・PC教室・プログラミング教室・学習タブレットの5カテゴリ。各カテゴリ代表数件を抜粋（PHP版はカテゴリごとに機械的な自動生成の穴埋め行を含め50件までパディングしているが、今回は実データのみを抜粋移植——正直に開示するスコープ縮小）。</p>
{learning_sections}
</div>

<div class="card" id="aruaru-eikaiwa-top50">
<h2 style="color:#34d399;">🌏 スマホ・タブレット・PC 優れた英会話アプリ・サイト TOP50</h2>
<p style="opacity:.72;font-size:15px;">📅 最終更新: {eikaiwa_updated} ／ 週1回（7日TTL）Cron自動更新</p>
{eikaiwa_table}
</div>

<div class="card" style="border-color:#6d28d9;">
<h3 style="color:#c4b5fd;">🎓 未経験から、無料IT研修＆無料の転職エージェントサービス</h3>
<p style="font-size:15px;color:#ede9fe;">プログラミング未経験から、受講料無料のIT研修を受けながら正社員を目指せるサービスや、無料で使える転職エージェントの一例検索です。</p>
<p><a href="{it_kenshu_url}" target="_blank" rel="noopener noreferrer">未経験　無料IT研修　無料 転職エージェント サービス を Google で開く</a></p>
</div>

<div class="card" style="border-color:#2d5a28;">
<h3 style="color:#bef264;">🏗️ 未経験から大工・造作大工・型枠大工・木造建築大工のリンクはこちら</h3>
<p style="font-size:15px;color:#ecfccb;">大工見習い〜専門工種への求人の一例検索です。条件は求人票・協会・訓練施設ごとに異なります。</p>
<p><a href="{daiku_url}" target="_blank" rel="noopener noreferrer">未経験から大工　造作大工　型枠大工　木造建築大工 を Google で開く</a></p>
</div>

<div class="card" style="border-color:#3b5998;">
<h3 style="color:#93c5fd;">📐 未経験・無資格の大工・CAD・施設管理・建築現場管理から、将来 一級建築士などを目指せる求人</h3>
<p style="font-size:15px;color:#dbeafe;">未経験・無資格の大工やCADオペレーター、施設管理、建築現場管理からスタートし、将来は一級建築士・木造建築士・管理建築士・1級建築施工管理技士などの資格取得を目指せる求人の一例検索です。</p>
<p><a href="{kenchiku_kanri_url}" target="_blank" rel="noopener noreferrer">未経験・無資格から 一級建築士・木造建築士・管理建築士・1級建築施工管理技士 を目指せる求人 を Google で開く</a></p>
</div>

<div class="notice">
  <span>💃</span>
  <span>キャバクラ・TVチャットレディなど主に女性向けのお仕事情報は移転しました →</span>
  <a href="/aruaru-lady">audiocafe.tokyo/aruaru-lady</a>
</div>

</div>

<footer>
  <p><a href="{ARUARU_TOKYO_URL}" target="_blank" rel="noopener noreferrer">🎲 aruaru.tokyo</a> — 「あるある」まとめ & 開発リポジトリ紹介</p>
  <p style="margin-top:6px;">© audiocafe.tokyo / aruaru — 掲載情報は外部サイトへのリンクです。各社の公式情報を必ずご確認ください。</p>
</footer>
</div>
"##,
        doda_updated_note = if doda_updated.is_empty() { String::new() } else { format!(" ／ 📅 {doda_updated}") },
        ext_count = ARUARU_EXT_SITES.len(),
    )
}

/// PHP側の`rakuten-mobile/index.php`(917行、`rm_render_fragment()`)が
/// 実際に表示している専用ページの内容をそのまま移植する。汎用JSONダンプ
/// (`render_value_generic`)ではPHP版と全く別のページになってしまうため
/// (2026-07-19監査で判明)、この関数はPHP版のセクション構成・見出し・
/// 静的マーケティング文言を1対1で再現しつつ、データ部分(料金・国際通話・
/// プラチナバンド/衛星)は既存の`fetch_cache`アーキテクチャ経由で取得した
/// 3キャッシュJSONから埋め込む。
/// PHP版`rakuten-mobile/index.php`(917行)の`<style>`ブロック
/// (448〜625行目)から、レイアウトの核となるダークテーマCSSを移植した
/// もの(`.rakuten-mobile-page`でスコープし、他ページの同名セレクタと
/// 衝突しないようにしている)。開閉パネル用のJS演出CSS(386〜412行目、
/// 埋め込みモード限定の機能)は対象外(ユーザー指示によるスコープ拡大:
/// 2026-07-19、見た目もPHP版と一致させる)。
const RAKUTEN_MOBILE_STYLE: &str = r#"<style>
.rakuten-mobile-page{--bg:#0b1220;--surface:rgba(15,23,42,.9);--border:rgba(148,163,184,.2);--red:#ef4444;--blue:#3b82f6;--cyan:#22d3ee;--purple:#a78bfa;--orange:#fb923c;--text:#e2e8f0;--dim:#94a3b8;--yellow:#fde68a;margin:-2rem -1rem;background:var(--bg);color:var(--text);font-family:'Noto Sans JP',system-ui,sans-serif;line-height:1.7}
.rakuten-mobile-page a{color:#7dd3fc;text-decoration:underline}
.rakuten-mobile-page a:hover{color:#bae6fd}
.rakuten-mobile-page .page-wrap{max-width:1100px;margin:0 auto;padding:24px 16px 40px}
.rakuten-mobile-page .rm-hero{background:linear-gradient(135deg,rgba(231,10,38,.18) 0%,rgba(59,130,246,.1) 100%);border:2px solid rgba(239,68,68,.4);border-radius:18px;padding:28px 24px;margin-bottom:24px}
.rakuten-mobile-page .rm-hero__badge{display:inline-flex;align-items:center;gap:6px;padding:4px 12px;border-radius:999px;background:rgba(239,68,68,.2);border:1px solid rgba(239,68,68,.5);color:#fca5a5;font-size:13px;font-weight:700;margin-bottom:12px}
.rakuten-mobile-page .rm-hero h1{font-size:clamp(1.3rem,4vw,2rem);font-weight:900;color:#fff;margin-bottom:8px;line-height:1.3}
.rakuten-mobile-page .rm-hero__sub{font-size:15px;color:var(--dim);margin-bottom:16px}
.rakuten-mobile-page .rm-hero__price{font-size:clamp(1.6rem,5vw,2.4rem);font-weight:900;color:var(--red);letter-spacing:-.02em}
.rakuten-mobile-page .rm-hero__price span{font-size:.6em;color:var(--dim)}
.rakuten-mobile-page .rm-hero__official{display:inline-block;margin-top:12px;padding:8px 18px;border-radius:10px;background:rgba(239,68,68,.25);border:1px solid rgba(239,68,68,.6);color:#fca5a5;font-weight:800;font-size:15px;text-decoration:none}
.rakuten-mobile-page .rm-hero__official:hover{background:rgba(239,68,68,.4);color:#fff}
.rakuten-mobile-page .rm-cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(290px,1fr));gap:14px;margin-bottom:24px}
.rakuten-mobile-page .rm-card{padding:18px 20px;border-radius:14px;background:rgba(2,6,23,.85)}
.rakuten-mobile-page .rm-card--intl{border:1px solid rgba(59,130,246,.4)}
.rakuten-mobile-page .rm-card--sat{border:1px solid rgba(56,189,248,.4)}
.rakuten-mobile-page .rm-card--plat{border:1px solid rgba(167,139,250,.4)}
.rakuten-mobile-page .rm-card__h3{color:#fff;font-size:16px;font-weight:800;margin-bottom:6px}
.rakuten-mobile-page .rm-card__meta{color:var(--dim);font-size:13px;margin-bottom:8px}
.rakuten-mobile-page .rm-card__body{font-size:14px;line-height:1.8;color:var(--text)}
.rakuten-mobile-page .rm-card__body .dim{color:var(--dim);font-size:13px}
.rakuten-mobile-page .rm-area{background:rgba(15,23,42,.5);border:1px solid var(--border);border-radius:12px;padding:16px 18px;margin-bottom:24px}
.rakuten-mobile-page .rm-area__h{font-size:15px;font-weight:800;color:var(--dim);margin-bottom:10px}
.rakuten-mobile-page .rm-area__btns{display:flex;flex-wrap:wrap;gap:8px}
.rakuten-mobile-page .rm-area__btn{display:inline-block;padding:6px 14px;border-radius:8px;font-size:15px;font-weight:700;text-decoration:none}
.rakuten-mobile-page .rm-area__btn--red{background:rgba(231,10,38,.2);border:1px solid rgba(231,10,38,.5);color:#fca5a5}
.rakuten-mobile-page .rm-area__btn--blue{background:rgba(59,130,246,.15);border:1px solid rgba(59,130,246,.4);color:#93c5fd}
.rakuten-mobile-page .rm-area__btn--red:hover{background:rgba(231,10,38,.35);color:#fff}
.rakuten-mobile-page .rm-area__btn--blue:hover{background:rgba(59,130,246,.3);color:#fff}
.rakuten-mobile-page .rm-coverage{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:10px;margin-bottom:24px}
.rakuten-mobile-page .rm-coverage__item{background:rgba(15,23,42,.55);border-radius:10px;padding:12px 14px}
.rakuten-mobile-page .rm-coverage__item--cyan{border:1px solid rgba(34,211,238,.3)}
.rakuten-mobile-page .rm-coverage__item--purple{border:1px solid rgba(139,92,246,.3)}
.rakuten-mobile-page .rm-coverage__item--orange{border:1px solid rgba(251,146,60,.3)}
.rakuten-mobile-page .rm-coverage__ttl{font-weight:800;font-size:15px;margin-bottom:6px}
.rakuten-mobile-page .rm-coverage__ttl--cyan{color:#22d3ee}
.rakuten-mobile-page .rm-coverage__ttl--purple{color:#a78bfa}
.rakuten-mobile-page .rm-coverage__ttl--orange{color:#fb923c}
.rakuten-mobile-page .rm-coverage__body{font-size:15px;line-height:1.7;color:var(--text)}
.rakuten-mobile-page .rm-links{background:linear-gradient(180deg,rgba(191,219,254,.1),rgba(17,8,24,.6));border:1px solid rgba(56,189,248,.25);border-radius:14px;padding:20px 20px;margin-bottom:24px}
.rakuten-mobile-page .rm-links__h2{font-size:clamp(1.05rem,3.2vw,1.25rem);font-weight:800;color:#7dd3fc;margin-bottom:8px}
.rakuten-mobile-page .rm-links__lead{font-size:15px;line-height:1.75;color:var(--text);opacity:.95;margin-bottom:12px}
.rakuten-mobile-page .rm-links__list{list-style:none;padding:0;margin:0 0 12px;line-height:1.9;font-size:15px;color:#e0f2fe}
.rakuten-mobile-page .rm-links__list a{color:#7dd3fc;font-weight:700;text-decoration:underline}
.rakuten-mobile-page .rm-links__note{font-size:15px;line-height:1.65;color:#cbd5e1;opacity:.9}
.rakuten-mobile-page .rm-search-btns{display:flex;flex-wrap:wrap;gap:8px;margin-bottom:24px}
.rakuten-mobile-page .rm-search-btn{display:inline-block;padding:8px 16px;border-radius:9px;font-size:15px;font-weight:800;text-decoration:none}
.rakuten-mobile-page .rm-search-btn--red{background:rgba(231,10,38,.25);border:1px solid rgba(231,10,38,.6);color:#fca5a5}
.rakuten-mobile-page .rm-search-btn--blue{background:rgba(59,130,246,.15);border:1px solid rgba(59,130,246,.4);color:#93c5fd}
.rakuten-mobile-page .rm-search-btn--purple{background:rgba(139,92,246,.15);border:1px solid rgba(139,92,246,.4);color:#c4b5fd}
.rakuten-mobile-page .rm-search-btn:hover{opacity:.8;color:#fff}
.rakuten-mobile-page .rm-cron-note{background:rgba(15,23,42,.5);border:1px solid var(--border);border-radius:10px;padding:14px 16px;margin-bottom:24px;font-size:13px;color:var(--dim);line-height:1.7}
.rakuten-mobile-page .rm-cron-note code{background:#0a1628;padding:2px 7px;border-radius:4px;color:#a5f3fc;font-size:12px}
.rakuten-mobile-page .rm-footer{text-align:center;padding:24px 16px;color:var(--dim);font-size:14px;border-top:1px solid var(--border);margin-top:16px}
.rakuten-mobile-page .rm-footer a{color:var(--dim)}
.rakuten-mobile-page .rm-footer a:hover{color:var(--text)}
</style>"#;

async fn render_rakuten_mobile_body() -> String {
    let rk = fetch_cache("rakuten-mobile-cache.json").await;
    let intl = fetch_cache("rakuten-intl-call-cache.json").await;
    let plat = fetch_cache("rakuten-platinum-cache.json").await;

    let empty = Value::Null;
    let rk = rk.as_ref().unwrap_or(&empty);
    let intl = intl.as_ref().unwrap_or(&empty);
    let plat = plat.as_ref().unwrap_or(&empty);

    let price = get_str(rk, "price", "最大3,278円（税込）");
    let updated_at = get_str(rk, "updated_at", "");
    let official_url = "https://network.mobile.rakuten.co.jp/fee/saikyo-plan/";

    let intl_price = get_str(intl, "intl_plan_price_ja", "月980円（税込）");
    let intl_name = get_str(intl, "intl_plan_name_ja", "国際通話かけ放題");
    let intl_count = get_str(intl, "intl_countries_count", "66");
    let intl_crawled = get_str(intl, "crawled_at", "");
    let intl_ok = get_bool(intl, "crawl_success");
    let intl_free_url = "https://network.mobile.rakuten.co.jp/service/international-call-free/";

    let plat_status = get_str(plat, "platinum_status_ja", "700MHz帯プラチナバンドを整備中。地下・屋内・山間部でのつながりやすさを改善。");
    let plat_detail = get_str(plat, "platinum_detail_ja", "700MHz帯（プラチナバンド）は電波が建物内や地下街まで届きやすい低周波数帯。屋内での通話・データ通信の安定性が向上。");
    let plat_coverage = get_str(plat, "platinum_coverage_ja", "全国整備進行中（順次拡大中）");
    let sat_status = get_str(plat, "satellite_status_ja", "AST SpaceMobile との提携により、衛星ブロードバンド通話サービスを準備中。");
    let sat_detail = get_str(plat, "satellite_detail_ja", "低軌道衛星（LEO）を利用し、山間部・離島・海上でも通常のスマートフォンで通話・データ通信が可能になる見込み。");
    let sat_launch = get_str(plat, "satellite_launch_ja", "商用サービス開始時期は未定（2025〜2026年を目標と報道あり）");
    let plat_crawled = get_str(plat, "crawled_at", "");
    let plat_ok = get_bool(plat, "crawl_success");

    let area_url = "https://network.mobile.rakuten.co.jp/area/";
    let area_faq_url = "https://network.mobile.rakuten.co.jp/faq/detail/00001549/";

    let we2plus_url = google_search_url("楽天モバイル お勧め 1円 高性能CPU 富士通製 スマホ we2 plus");
    let packet_url = google_search_url("楽天モバイル パケット放題 プラン");
    let phone_url = google_search_url("楽天モバイル 電話放題 楽天リンク");
    let link_android_url = google_search_url("楽天モバイル スマホアプリの名前は 楽天リンク Android版");
    let link_iphone_url = google_search_url("楽天モバイル スマホアプリの名前は 楽天リンク iPhone版");
    let campaign_url = google_search_url("楽天モバイル 乗り換え キャンペーン");
    let price_search_url = google_search_url("楽天モバイル Rakuten最強プラン 料金");

    format!(
        r##"{RAKUTEN_MOBILE_STYLE}
<div class="rakuten-mobile-page">
<div class="page-wrap">

<div class="rm-hero">
<div class="rm-hero__badge">📶 Rakuten最強プラン</div>
<h1>楽天モバイル 最新情報</h1>
<p class="rm-hero__sub">自社の楽天回線エリアと au回線（パートナー回線）エリアを合わせてデータ使い放題（パケット放題）となります。</p>
<div>月間無制限に使っても <span class="rm-hero__price">{price}<span>（税込）</span></span></div>
<a href="{official_url}" target="_blank" rel="noopener noreferrer" class="rm-hero__official">公式サイトで確認 →</a>
<p style="font-size:13px;color:var(--dim);margin-top:10px;">📅 {updated_at}</p>
</div>

<div class="rm-coverage">
<div class="rm-coverage__item rm-coverage__item--cyan">
<div class="rm-coverage__ttl rm-coverage__ttl--cyan">📡 楽天回線エリア</div>
<p class="rm-coverage__body">人口カバー率<strong style="color:var(--yellow)">99.9%</strong>を達成。自社基地局エリア内ではデータ高速<strong style="color:var(--yellow)">無制限</strong>で利用できます。</p>
</div>
<div class="rm-coverage__item rm-coverage__item--purple">
<div class="rm-coverage__ttl rm-coverage__ttl--purple">🔄 パートナー回線（au）エリア</div>
<p class="rm-coverage__body">楽天電波が届きにくい屋内や一部エリアでは<strong style="color:#a78bfa">auローミング</strong>を利用。月間<strong style="color:#fca5a5">5GBまで</strong>高速、超過後は最大1Mbps。</p>
</div>
<div class="rm-coverage__item rm-coverage__item--orange">
<div class="rm-coverage__ttl rm-coverage__ttl--orange">⚠️ 注意点</div>
<p class="rm-coverage__body">地下・高層ビル・奥まった屋内では繋がりにくい場合あり。プラチナバンド（700MHz帯）を拡大中。</p>
</div>
</div>

<div class="rm-area">
<div class="rm-area__h">🗺️ エリア確認ツール</div>
<div class="rm-area__btns">
<a href="{area_url}" target="_blank" rel="noopener noreferrer" class="rm-area__btn rm-area__btn--red">📍 楽天モバイル 通信・エリアマップ</a>
<a href="{area_faq_url}" target="_blank" rel="noopener noreferrer" class="rm-area__btn rm-area__btn--blue">❓ データ高速無制限エリアとは</a>
</div>
</div>

<div class="rm-search-btns">
<a href="{price_search_url}" target="_blank" rel="noopener noreferrer" class="rm-search-btn rm-search-btn--red">🔍 最新料金を Google で検索</a>
<a href="{campaign_url}" target="_blank" rel="noopener noreferrer" class="rm-search-btn rm-search-btn--blue">🔍 乗り換えキャンペーン</a>
<a href="{we2plus_url}" target="_blank" rel="noopener noreferrer" class="rm-search-btn rm-search-btn--purple">📱 1円スマホ（we2 plus）</a>
</div>

<div class="rm-cards">
<div class="rm-card rm-card--intl">
<div class="rm-card__h3">📞 楽天モバイル 国際通話プラン詳細</div>
<p class="rm-card__meta">📅 {intl_crawled}{intl_ok_note}</p>
<div class="rm-card__body">
🇯🇵 日本 → 海外 プラン料金：{intl_price} / {intl_name}<br>
🌍 かけ放題対象国：{intl_count} カ国<br>
✈️ 海外 → 日本：Rakuten Link 利用時 無料（対象国・条件あり）<br><br>
<strong style="color:var(--yellow)">🌏 海外からも日本へ電話放題？</strong><br>
✅ はい、かなり本当です。主に Rakuten Link アプリ利用時（条件あり）。<br><br>
🇯🇵 日本→日本：Rakuten Link で無料<br>
🇯🇵 日本→海外：「{intl_name}（{intl_price}）」で{intl_count}カ国かけ放題<br>
✈️ 海外→日本：Rakuten Link で無料（対象国から）<br><br>
<a href="{intl_free_url}" target="_blank" rel="noopener noreferrer">📎 国際通話かけ放題 公式ページ</a>
</div>
</div>

<div class="rm-card rm-card--sat">
<div class="rm-card__h3">🚀 衛星ブロードバンド通話（AST SpaceMobile 提携）</div>
<p class="rm-card__meta">📅 {plat_crawled}{plat_ok_note}</p>
<div class="rm-card__body">{sat_status}<br><span style="color:#cbd5e1">{sat_detail}</span><br><span class="dim">🛰️ {sat_launch}</span></div>
</div>

<div class="rm-card rm-card--plat">
<div class="rm-card__h3">📡 プラチナ回線（700MHz帯 プラチナバンド）</div>
<p class="rm-card__meta">📅 {plat_crawled}{plat_ok_note}</p>
<div class="rm-card__body">{plat_status}<br><span style="color:#cbd5e1">{plat_detail}</span><br><span class="dim">📶 カバレッジ：{plat_coverage}</span></div>
</div>
</div>

<div class="rm-links">
<h2 class="rm-links__h2">📶 楽天モバイル（1円スマホ・パケット放題・電話放題）</h2>
<p class="rm-links__lead">スマホなら楽天モバイルへの乗り換えを検討できます。eSIM 対応端末やキャンペーンの一例として、富士通製「we2 plus」など高性能 CPU 端末を<strong>1円</strong>で入手できる案内が出る場合があります（時期・在庫・契約条件は要確認）。日本全国で<strong>楽天リンク</strong>アプリが使えます。</p>
<ul class="rm-links__list">
<li><a href="{we2plus_url}" target="_blank" rel="noopener noreferrer">1円スマホの例：富士通製 we2 plus など（要確認）</a></li>
<li><a href="{packet_url}" target="_blank" rel="noopener noreferrer">パケット放題・データ使い放題プラン</a></li>
<li><a href="{phone_url}" target="_blank" rel="noopener noreferrer">電話放題・楽天リンク経由の通話</a></li>
<li><a href="{link_android_url}" target="_blank" rel="noopener noreferrer">楽天リンク Android版</a></li>
<li><a href="{link_iphone_url}" target="_blank" rel="noopener noreferrer">楽天リンク iPhone版</a></li>
<li><a href="{campaign_url}" target="_blank" rel="noopener noreferrer">楽天モバイル 乗り換え・キャンペーン全般</a></li>
</ul>
<p class="rm-links__note">楽天モバイルのアンテナ・基地局が届くエリアでは、オンライン配信や TV チャットでも<strong>パケットを気にしにくいプラン</strong>を検討できます。病院などの FREE Wi-Fi が使える場合、在宅・入院中の環境づくりにも役立つことがあります（プラン内容・エリアは必ず公式で確認してください）。</p>
</div>

<div class="rm-cron-note">
<strong style="color:#67e8f9">⏱ 自動更新について</strong><br>
このページの元データ(楽天モバイル料金・国際通話・プラチナバンド)はPHP版サイトが毎朝05:00AMに自動クロール・キャッシュ更新しています。キャッシュ先: <code>rakuten-mobile-cache.json</code> 他2ファイル。
</div>

<div style="display:flex;flex-wrap:wrap;gap:10px;justify-content:center;margin-bottom:24px">
<a href="/" style="display:inline-block;padding:8px 18px;border-radius:10px;background:rgba(15,23,42,.7);border:1px solid var(--border);color:var(--dim);font-weight:700;text-decoration:none;">← audiocafe.tokyo トップ</a>
<a href="/aruaru" style="display:inline-block;padding:8px 18px;border-radius:10px;background:rgba(15,23,42,.7);border:1px solid var(--border);color:var(--dim);font-weight:700;text-decoration:none;">📊 aruaru（IT技術情報）</a>
<a href="/aruaru-lady" style="display:inline-block;padding:8px 18px;border-radius:10px;background:rgba(15,23,42,.7);border:1px solid var(--border);color:var(--dim);font-weight:700;text-decoration:none;">💃 aruaru-lady（女性向け情報）</a>
</div>

<div class="rm-footer">
楽天モバイル情報は毎朝05:00AMに自動クロール更新。内容は必ず<a href="{official_url}" target="_blank" rel="noopener noreferrer">公式サイト</a>でご確認ください。 ・
<a href="{ARUARU_TOKYO_URL}" target="_blank" rel="noopener noreferrer">🎲 aruaru.tokyo</a>
</div>

</div>
</div>
"##,
        intl_ok_note = if intl_ok { " ✓ クロール成功" } else { "" },
        plat_ok_note = if plat_ok { " ✓ クロール成功" } else { "" },
    )
}

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

fn healthz_response() -> hyper_compat::Response {
    hyper::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; charset=utf-8")
        .body(hyper_compat::fixed_body(bytes::Bytes::from_static(b"ok")))
        .expect("building a response from a fixed set of valid headers cannot fail")
}

/// PHP版トップページの実データ本体。実ページの`var L=[...]`(147件、
/// `index.php` 1609〜1757行目)を、Node.js(`new Function('return ('+src+')')`)
/// で一度JSとして評価して`JSON.stringify`し、`assets/top_languages.json`
/// (147件全件、フィールドの取捨選択なし)として保存したものを`include_str!`で
/// 埋め込み、起動時に一度だけデシリアライズする(2026-07-19、完全版へ拡張)。
/// フィールド対応: `g`=Google翻訳用言語コード、`n`=英語名、`t`=現地語表記、
/// `a`=カードラベル(国名等)、`r`=地域、`c`=英語エッセイ本文(全147件に存在、
/// 短い1行のみのカードもあれば政治・宗教・地政学的な主張を含む長文のカードも
/// ある)、`d`=日本語エッセイ本文(10件のみ存在)、`p`=正式国名(1件のみ存在)、
/// `cardLinks`=カード別関連リンク(8件のみ存在)、`fc`=flagcdn国コード。
#[derive(Debug, Clone, Deserialize)]
struct CardLink {
    href: String,
    label: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LangCard {
    g: String,
    n: String,
    t: String,
    a: String,
    r: String,
    c: String,
    #[serde(default)]
    d: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    p: Option<String>,
    #[serde(default, rename = "cardLinks")]
    card_links: Option<Vec<CardLink>>,
    fc: String,
}

static TOP_LANGUAGES: Lazy<Vec<LangCard>> = Lazy::new(|| {
    serde_json::from_str::<Vec<LangCard>>(include_str!("../assets/top_languages.json"))
        .expect("assets/top_languages.json must be valid JSON matching LangCard (147 entries)")
});

/// PHP版`SEARCH_SERIES`(`index.php` 2566行目)をNode.jsの
/// `new Function('return ('+src+')')`でJSとして直接評価しlosslessに抽出した
/// もの(`assets/search_series.json`)。PHP版コメントは「84件」と記載していたが
/// (前回HANDOFF参照、未検証の見積もり)、実際にlosslessな評価で数えたところ
/// **77件**だった(2026-07-19、YouTube再生リストのシリーズ機能復活時に判明・
/// 訂正)。各シリーズは`btn`(検索クエリ/ボタンラベル)・`label`(短縮表示用、
/// 空文字のこともある)・`urls`(1件以上のYouTube URL、`watch`/`shorts`/
/// `youtu.be`/`results?search_query`のいずれか)を持つ。
#[derive(Debug, Clone, Deserialize)]
struct SearchSeries {
    btn: String,
    #[serde(default)]
    label: String,
    urls: Vec<String>,
}

static SEARCH_SERIES: Lazy<Vec<SearchSeries>> = Lazy::new(|| {
    serde_json::from_str::<Vec<SearchSeries>>(include_str!("../assets/search_series.json"))
        .expect("assets/search_series.json must be valid JSON matching SearchSeries")
});

/// PHP版トップページの実際のCSS(`<style>`ブロック冒頭、`:root`変数・
/// `.header`/`.card`系クラス)を`.top-page`配下にスコープして移植。
/// 元CSSはbody直下の裸セレクタだったため、既存の他ページ(`ARUARU_STYLE`等)と
/// 同じ手法で`.top-page`プレフィックスを付与した。2026-07-19、完全版への
/// 拡張に伴い、エッセイ本文・カードリンク・言語別導線リンク・検索フォーム・
/// 地域ピル・YouTube背景プレイヤー・壁紙コーナー用のクラスを追加した。
const TOP_STYLE: &str = r#"<style>
.top-page{--bg:#000;--surface:rgba(15,23,42,.52);--border:rgba(255,255,255,.06);--text:#e2e8f0;--text-dim:#94a3b8;--text-muted:#64748b;--cyan:#22d3ee;--cyan-glow:rgba(34,211,238,.15);margin:-2rem -1rem;background:var(--bg);color:var(--text);font-family:'Segoe UI',system-ui,-apple-system,sans-serif;line-height:1.6}
.top-page a{color:#7dd3fc}
.top-page .header{position:relative;overflow:hidden;text-align:center;padding:2rem 1rem 2rem}
.top-page .logo{display:inline-block;font-size:clamp(2rem,6vw,3.2rem);font-weight:900;letter-spacing:-.02em;background:linear-gradient(90deg,#6366f1,#22c55e,#facc15,#ff1493,#7c3aed,#6366f1);background-size:400% 100%;-webkit-background-clip:text;background-clip:text;color:transparent}
.top-page .subtitle{margin-top:.5rem;font-size:clamp(.95rem,2vw,1.25rem);color:#fff;font-weight:500}
.top-page .note{margin-top:.5rem;font-size:.8rem;color:#fff;max-width:36rem;margin-left:auto;margin-right:auto;line-height:1.6}
.top-page .lang-select-link{margin-top:.5rem}
.top-page .blog-link{margin-top:.4rem;font-size:.85rem}
.top-page .blog-link a{color:#7dd3fc;text-decoration:none;font-weight:600}
.top-page .lang-select-link a{color:#fff;font-weight:600}
.top-page .main{max-width:76rem;margin:0 auto;padding:1rem 1rem 3rem;width:100%}
.top-page .region-title{font-size:1.4rem;font-weight:700;color:#fff;margin:1.6rem 0 .8rem;letter-spacing:.03em}
.top-page .grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,220px),1fr));gap:.65rem}
.top-page .card{display:flex;flex-direction:column;align-items:center;gap:.35rem;padding:1rem .55rem;border-radius:.75rem;border:1px solid var(--border);background:var(--surface);backdrop-filter:blur(6px);text-align:center}
.top-page .card-flag{width:60px;height:40px;object-fit:cover;border-radius:4px;box-shadow:0 2px 8px rgba(0,0,0,.3)}
.top-page .card-code{display:block;font-size:.95rem;font-weight:800;letter-spacing:.08em;color:var(--cyan)}
.top-page .card-native{display:block;font-size:.95rem;font-weight:600;color:rgba(255,255,255,.9)}
.top-page .card-country{display:block;font-size:.85rem;font-weight:500;color:rgba(255,255,255,.75)}
.top-page .card-essay{text-align:left;font-size:1.15rem;color:var(--text-dim);max-height:9.5rem;overflow-y:auto;margin-top:.3rem;line-height:1.55}
.top-page .card-essay p{margin:0 0 .5rem}
.top-page .card-essay-label{display:block;font-size:.68rem;font-weight:800;letter-spacing:.08em;color:#7dd3fc;text-transform:uppercase;margin-bottom:.15rem}
.top-page .card-essay-d{border-top:1px dashed var(--border);margin-top:.5rem;padding-top:.5rem;font-size:1.15rem}
.top-page .card-links{list-style:none;padding:0;margin:.4rem 0 0;text-align:left;font-size:.72rem;line-height:1.6}
.top-page .card-links li{margin-bottom:.15rem}
.top-page .card-actions{display:flex;flex-wrap:wrap;gap:.3rem;justify-content:center;margin-top:.5rem}
.top-page .card-actions a{font-size:.68rem;padding:.15rem .45rem;border:1px solid var(--border);border-radius:999px;background:rgba(34,211,238,.08)}
.top-page .nav-box{background:rgba(15,23,42,.5);border:1px solid var(--border);border-radius:12px;padding:16px 18px;margin:1.5rem 0}
.top-page .nav-box h2{color:#7dd3fc;font-size:1.05rem;margin-bottom:.6rem}
.top-page .nav-box ul{list-style:none;padding:0;margin:0;line-height:1.9}
.top-page .footer{border-top:1px solid rgba(255,255,255,.05);padding:1.5rem 1rem;text-align:center;font-size:.75rem;color:var(--text-muted);line-height:1.7}
.top-page .search-wrap{display:flex;justify-content:center;gap:.4rem;margin:1rem auto 0;max-width:26rem}
.top-page .search-wrap input{flex:1;padding:.5rem .8rem;border-radius:999px;border:1px solid var(--border);background:rgba(255,255,255,.05);color:var(--text)}
.top-page .search-wrap button{padding:.5rem 1rem;border-radius:999px;border:1px solid var(--cyan);background:rgba(34,211,238,.12);color:var(--cyan);font-weight:700}
.top-page .pills{display:flex;flex-wrap:wrap;justify-content:center;gap:.4rem;margin-top:.7rem}
.top-page .pill{padding:.3rem .8rem;border-radius:999px;border:1px solid var(--border);font-size:1.15rem;background:rgba(255,255,255,.04)}
.top-page .pill.is-active{border-color:var(--cyan);color:var(--cyan);background:var(--cyan-glow)}
.top-page .yt-bg-player{max-width:640px;margin:1rem auto;border-radius:12px;overflow:hidden;border:1px solid var(--border);background:rgba(15,23,42,.85);padding-bottom:.5rem;position:relative}
.top-page .yt-panel-close{position:sticky;top:0;z-index:2;width:100%;border:none;background:rgba(15,23,42,.95);color:#22d3ee;font-weight:800;padding:.4rem;cursor:pointer;letter-spacing:.06em}
.top-page .yt-panel-open{position:fixed;top:1rem;right:1rem;z-index:999;font-weight:800;padding:.5rem 1rem;border-radius:14px;border:2px solid rgba(34,211,238,.8);background:rgba(15,23,42,.95);color:#22d3ee;cursor:pointer;letter-spacing:.06em}
.top-page .yt-now-playing{display:block;padding:.4rem .8rem;font-size:.8rem;color:#fff;text-decoration:underline;cursor:pointer}
.top-page .yt-series-controls{padding:0 .8rem .4rem}
.top-page .yt-next-btn{border:1px solid var(--border);border-radius:999px;background:rgba(34,211,238,.12);color:#22d3ee;padding:.3rem .8rem;font-size:.75rem;cursor:pointer}
.top-page .yt-series-list{max-height:180px;overflow-y:auto;display:flex;flex-wrap:wrap;gap:.3rem;padding:.4rem .8rem}
.top-page .yt-series-btn{border:1px solid var(--border);border-radius:999px;background:transparent;color:#fff;padding:.25rem .7rem;font-size:1.05rem;cursor:pointer}
.top-page .yt-series-btn.is-active{background:rgba(34,211,238,.2);border-color:#22d3ee;color:#22d3ee}
.top-page .yt-wp-corner{max-width:640px;margin:1rem auto 0;background:rgba(15,23,42,.5);border:1px solid var(--border);border-radius:12px;padding:1rem}
.top-page #logoWrap{max-width:640px;margin:1rem auto 0;padding:.6rem;text-align:center;background:rgba(15,23,42,.5);border:1px solid var(--border);border-radius:12px;overflow:hidden}
.top-page #acLogoCanvas{width:100%;height:200px;display:block}
.top-page #logoModeBar,.top-page #danceFixedBar{display:flex;flex-wrap:wrap;justify-content:center;gap:.35rem;margin-top:.8rem}
.top-page #danceFixedBar{display:none;margin-top:.5rem}
.top-page #danceFixedBar.is-visible{display:flex}
.top-page .df-label{font-size:.75rem;color:var(--text-dim);align-self:center;margin-right:.2rem}
.top-page .logo-mode-btn,.top-page .dance-fixed-btn{border:1px solid var(--border);border-radius:999px;background:transparent;color:var(--text-dim);padding:.3rem .8rem;font-size:.8rem;cursor:pointer}
.top-page .logo-mode-btn.active,.top-page .dance-fixed-btn.active{background:rgba(34,211,238,.2);border-color:#22d3ee;color:#22d3ee}
.top-page .yt-wp-head{display:block;font-weight:800;color:#fde68a}
.top-page .yt-wp-hint{display:block;font-size:.75rem;color:var(--text-muted);margin:.2rem 0 .6rem}
.top-page .yt-wp-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(120px,1fr));gap:.6rem}
.top-page .yt-wp-item{text-align:center;font-size:.72rem}
.top-page .yt-wp-thumb{width:100%;border-radius:8px}
.top-page .yt-wp-name{display:block;margin:.25rem 0}
.top-page .yt-wp-dl{display:inline-block;margin-top:.15rem;color:var(--cyan)}
</style>"#;

/// PHP版のcanvasパーティクルロゴ(`index.php` 5854行目以降)の描画式を
/// そのまま移植したJSエンジン。`danceMotionForChar`の10ジャンル分の式・
/// `drawTextGradient`のグラデーション定義・`getTextPixels`/`initParticles`の
/// パーティクル物理は原文と同じ計算式を使用している(YouTube音量連動の
/// `ytEnergy`は接続していないため常に0、Dance AIは音量パターン解析の
/// 代わりに10ジャンルの定期ローテーション、波モードの帆船装飾は省略——
/// いずれもこの定数のコメントとHANDOFFに明記済み)。
const LOGO_CANVAS_SCRIPT: &str = r#"<script>
(function(){
  var canvas = document.getElementById('acLogoCanvas');
  if(!canvas) return;
  var ctx = canvas.getContext('2d');
  var TEXT = 'audiocafe.tokyo';
  var GRAD_SPEED = 18;
  var CW = 1, CH = 1;

  function calcFontSize(w, h){ return Math.min(w * 0.93 / TEXT.length * 1.62, h * 0.70); }

  // PHP版`drawSailingShip()`(`index.php` 5973〜6146行目)をそのまま移植。
  // 波モードで、ロゴ本体の波の進行方向に合わせて帆船がヨー角を
  // ゆっくり切り替えながら3D風に揺れる装飾(2026-07-19、ユーザー指摘
  // 「PHP版で波は上に船が動いていたのを再現して」により追加、前回の
  // 「帆船装飾は省略」というスコープ縮小を撤回)。
  var shipYawDeg = 75;
  function drawSailingShip(ctx, now, W, H, amp, yawDeg){
    var leftPad = 8;
    var fs = calcFontSize(W, H);
    ctx.save();
    ctx.font = '900 ' + fs + "px 'Segoe UI',system-ui,sans-serif";
    var widthBeforeDot = ctx.measureText('audiocafe').width;
    ctx.restore();
    var shipCX = leftPad + widthBeforeDot / 2;
    var shipScale = Math.min(W, H) * 0.18;
    var cy = H * 0.28;

    var bobY = Math.sin(now * 1.1) * 6 * amp + Math.sin(now * 2.3) * 2 * amp;
    var bobX = Math.cos(now * 0.9) * 8 * amp + Math.cos(now * 1.8) * 3 * amp;
    var roll = (Math.sin(now * 1.1) * 0.12 + Math.sin(now * 2.3) * 0.04) * amp;

    var yaw = yawDeg * Math.PI / 180;
    var cosY = Math.cos(yaw), sinY = Math.sin(yaw);
    var hullW = 55;
    var hullWScaled = hullW * Math.abs(cosY);
    var sailDepth = sinY;

    ctx.save();
    ctx.translate(shipCX + bobX, cy + bobY);
    ctx.rotate(roll);
    ctx.scale(shipScale / 100, shipScale / 100);
    var sv = ctx.shadowBlur;

    ctx.beginPath();
    ctx.moveTo(-hullWScaled, 0);
    ctx.bezierCurveTo(-hullWScaled - 5 * Math.abs(cosY), 8, -hullWScaled * 0.7, 22, 0, 24);
    ctx.bezierCurveTo(hullWScaled * 0.7, 22, hullWScaled + 5 * Math.abs(cosY), 8, hullWScaled, 0);
    ctx.closePath();
    var hullLight = cosY > 0 ? '#9a5c22' : '#5c3010';
    ctx.fillStyle = hullLight;
    ctx.shadowColor = 'rgba(0,0,0,0.55)'; ctx.shadowBlur = 10;
    ctx.fill();
    ctx.strokeStyle = '#3a1a05'; ctx.lineWidth = 1.8;
    ctx.stroke();

    if (Math.abs(cosY) > 0.15) {
      ctx.beginPath();
      ctx.moveTo(-hullWScaled, 0); ctx.lineTo(hullWScaled, 0);
      ctx.strokeStyle = cosY > 0 ? '#c8803a' : '#7a4020';
      ctx.lineWidth = 2.5; ctx.shadowBlur = 0;
      ctx.stroke();
    }

    var bowDir = cosY >= 0 ? -1 : 1;
    ctx.beginPath();
    ctx.moveTo(bowDir * hullWScaled, -2);
    ctx.lineTo(bowDir * (hullWScaled + 18), -12);
    ctx.strokeStyle = '#8b5e2a'; ctx.lineWidth = 2.5; ctx.shadowBlur = 0;
    ctx.stroke();

    var mastX = sinY * 6;
    ctx.beginPath();
    ctx.moveTo(mastX, 2); ctx.lineTo(mastX, -80);
    ctx.strokeStyle = '#8b5e2a'; ctx.lineWidth = 3.5; ctx.shadowBlur = 0;
    ctx.stroke();

    var yardW = 36 * Math.abs(cosY) + 8;
    ctx.beginPath();
    ctx.moveTo(mastX - yardW, -40); ctx.lineTo(mastX + yardW, -40);
    ctx.strokeStyle = '#7a5020'; ctx.lineWidth = 2.5;
    ctx.stroke();

    var sailBow = Math.sin(now * 1.4) * 7 * amp * Math.abs(cosY);
    var sailW = yardW * 0.95;
    var sailCurve = sailDepth * 18 + sailBow;
    ctx.beginPath();
    ctx.moveTo(mastX - sailW, -38);
    ctx.bezierCurveTo(mastX - sailW * 0.5 + sailCurve, -22, mastX - sailW * 0.3 + sailCurve, -10, mastX - sailW * 0.2, 2);
    ctx.lineTo(mastX + sailW * 0.2, 2);
    ctx.bezierCurveTo(mastX + sailW * 0.3 + sailCurve, -10, mastX + sailW * 0.5 + sailCurve, -22, mastX + sailW, -38);
    ctx.closePath();
    var sailBrightness = 0.6 + 0.4 * Math.abs(cosY);
    var sg = ctx.createLinearGradient(mastX - sailW, -38, mastX + sailW, 2);
    sg.addColorStop(0, 'rgba(' + Math.round(255*sailBrightness) + ',' + Math.round(250*sailBrightness) + ',' + Math.round(230*sailBrightness) + ',0.95)');
    sg.addColorStop(1, 'rgba(' + Math.round(220*sailBrightness) + ',' + Math.round(205*sailBrightness) + ',' + Math.round(175*sailBrightness) + ',0.90)');
    ctx.fillStyle = sg;
    ctx.shadowColor = 'rgba(100,150,255,0.25)'; ctx.shadowBlur = 5;
    ctx.fill();
    ctx.strokeStyle = 'rgba(160,140,100,0.7)'; ctx.lineWidth = 1.2;
    ctx.stroke();

    var tsW = 18 * Math.abs(cosY) + 4;
    var tsBow = Math.sin(now * 1.8 + 0.5) * 5 * amp * Math.abs(cosY);
    var tsCurve = sailDepth * 10 + tsBow;
    ctx.beginPath();
    ctx.moveTo(mastX - tsW, -42);
    ctx.bezierCurveTo(mastX - tsW * 0.5 + tsCurve, -58, mastX + tsCurve * 0.5, -68, mastX, -78);
    ctx.bezierCurveTo(mastX + tsCurve * 0.5, -68, mastX + tsW * 0.5 + tsCurve, -58, mastX + tsW, -42);
    ctx.closePath();
    ctx.fillStyle = 'rgba(' + Math.round(245*sailBrightness) + ',' + Math.round(240*sailBrightness) + ',' + Math.round(220*sailBrightness) + ',0.88)';
    ctx.shadowBlur = 3;
    ctx.fill();
    ctx.strokeStyle = 'rgba(160,140,100,0.6)'; ctx.lineWidth = 1.0;
    ctx.stroke();

    var flagDir = cosY >= 0 ? 1 : -1;
    var flagWave = Math.sin(now * 3.5) * 5 * amp;
    ctx.beginPath();
    ctx.moveTo(mastX, -80);
    ctx.lineTo(mastX + flagDir * (16 + flagWave), -72);
    ctx.lineTo(mastX + flagDir * (14 + flagWave * 0.5), -66);
    ctx.lineTo(mastX, -72);
    ctx.closePath();
    ctx.fillStyle = '#ef4444';
    ctx.shadowColor = 'rgba(239,68,68,0.7)'; ctx.shadowBlur = 6;
    ctx.fill();

    ctx.beginPath();
    ctx.moveTo(mastX - sailW, -38); ctx.lineTo(-hullWScaled, 2);
    ctx.moveTo(mastX + sailW, -38); ctx.lineTo(hullWScaled, 2);
    ctx.strokeStyle = 'rgba(140,100,50,0.55)'; ctx.lineWidth = 1.0; ctx.shadowBlur = 0;
    ctx.stroke();

    ctx.shadowBlur = sv;
    ctx.restore();
  }

  function resize(){
    var dpr = Math.min(window.devicePixelRatio || 1, 4);
    CW = canvas.offsetWidth; CH = canvas.offsetHeight;
    canvas.width = Math.round(CW * dpr); canvas.height = Math.round(CH * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    initParticles();
  }

  var GRAD_STOPS = [
    [0,'#a855f7'],[0.08,'#3b82f6'],[0.20,'#22d3ee'],[0.32,'#22c55e'],[0.45,'#facc15'],
    [0.57,'#f97316'],[0.68,'#ef4444'],[0.80,'#ec4899'],[0.90,'#a855f7'],[1.00,'#3b82f6']
  ];

  function drawTextGradient(now, waveOffset){
    ctx.clearRect(0, 0, CW, CH);
    var fs = calcFontSize(CW, CH);
    ctx.font = '900 ' + fs + "px 'Segoe UI',system-ui,sans-serif";
    ctx.textAlign = 'left'; ctx.textBaseline = 'middle';
    var textW = ctx.measureText(TEXT).width;
    var leftPad = 8, baseY = CH / 2;
    var e = 0;
    var pulse = 1 + e * 0.05;
    var gradShift = (now * GRAD_SPEED / 360 * textW * 1.5) % textW;
    var grad = ctx.createLinearGradient(leftPad - gradShift, 0, leftPad - gradShift + textW * 1.5, 0);
    GRAD_STOPS.forEach(function(s){ grad.addColorStop(s[0], s[1]); });

    if (waveOffset) {
      var curX = leftPad;
      TEXT.split('').forEach(function(ch){
        var chW = ctx.measureText(ch).width;
        var ratio = (curX + chW / 2 - leftPad) / textW;
        var dy = waveOffset(ratio, now);
        ctx.save();
        ctx.shadowColor = 'rgba(168,85,247,0.6)'; ctx.shadowBlur = 18 + e * 18;
        ctx.fillStyle = grad;
        ctx.fillText(ch, curX, baseY + dy);
        ctx.restore();
        curX += chW;
      });
    } else {
      ctx.save();
      ctx.shadowColor = 'rgba(168,85,247,0.5)'; ctx.shadowBlur = 16 + e * 20;
      ctx.fillStyle = grad;
      ctx.translate(leftPad + textW / 2, baseY);
      ctx.scale(pulse, pulse);
      ctx.fillText(TEXT, -textW / 2, 0);
      ctx.restore();
    }
  }

  var DANCE_GENRE_NAMES = {
    kabuki:'🎭 歌舞伎', egypt:'🏛 エジプト', india:'🕉 インド', hiphop:'🎤 HIPHOP', kpop:'💖 K-POP',
    latin:'💃 ラテン', orchestra:'🎻 オーケストラ', jazz:'🎷 JAZZ', ethnic:'🪘 民族', freestyle:'✨ フリースタイル'
  };
  var DANCE_GENRE_COLORS = {
    kabuki:['#d71b3b','#1a1a1a','#ffd700','#ffffff'], egypt:['#ffd700','#d4a017','#3fb6bc','#8b4513'],
    india:['#ff6f00','#ff9933','#ec407a','#138808'], hiphop:['#ff0080','#8000ff','#00ffff','#ffff00'],
    kpop:['#ff6b9d','#c471ed','#12c2e9','#f7797d'], latin:['#ff0040','#ff8c00','#ffd700','#ff1493'],
    orchestra:['#4b0082','#9370db','#daa520','#f5f5dc'], jazz:['#2e1a47','#6a0dad','#d4af37','#4169e1'],
    ethnic:['#8b4513','#cd853f','#d2691e','#ff4500'], freestyle:['#00ffff','#ff00ff','#ffff00','#00ff00']
  };

  function danceMotionForChar(genre, now, i, ratio, beat){
    var dx = 0, dy = 0, rot = 0, scale = 1;
    if (genre === 'kabuki') {
      var pose = Math.floor(now * 0.5) % 4;
      var poseT = (now * 0.5) % 1;
      var freeze = poseT < 0.3 ? 1 : poseT > 0.7 ? 1 : Math.sin((poseT - 0.3) * Math.PI / 0.4);
      var tilt = [0, 0.15, -0.12, 0.2][pose];
      dy = Math.sin(ratio * Math.PI) * 10 * (1 - freeze) + beat * 6;
      rot = tilt * (1 - freeze * 0.7);
      scale = 1 + beat * 0.08;
    } else if (genre === 'egypt') {
      var step = Math.floor(now * 3);
      var stepT = (now * 3) % 1;
      dx = (step % 2 === 0 ? 1 : -1) * (8 + beat * 8) * Math.pow(stepT, 2);
      dy = -Math.abs(Math.sin(now * Math.PI * 2 + i * 0.5)) * (6 + beat * 6);
      scale = 1 + beat * 0.07;
    } else if (genre === 'india') {
      dx = Math.sin(now * 3 + i * 0.3) * (12 + beat * 10);
      dy = Math.cos(now * 2.5 + i * 0.2) * (8 + beat * 8);
      rot = Math.sin(now * 3) * 0.1;
      scale = 1 + Math.sin(now * 4 + i) * 0.08 + beat * 0.08;
    } else if (genre === 'hiphop') {
      var bounce = Math.pow(Math.abs(Math.sin(now * Math.PI * 3 + i * 0.4)), 3);
      dy = -bounce * (18 + beat * 20);
      dx = Math.sin(now * 8 + i) * (3 + beat * 6);
      rot = (Math.random() < 0.05 ? (Math.random() - 0.5) * 0.3 : 0) * (0.5 + beat);
      scale = 1 + bounce * 0.15 + beat * 0.12;
    } else if (genre === 'kpop') {
      var wave = Math.sin(ratio * Math.PI * 2 - now * 3.5);
      dy = wave * (14 + beat * 14);
      dx = Math.cos(ratio * Math.PI * 2 - now * 3.5) * (4 + beat * 4);
      rot = wave * 0.08;
      scale = 1 + Math.abs(wave) * 0.06 + beat * 0.08;
    } else if (genre === 'latin') {
      var hip = Math.sin(now * Math.PI * 2.5) * (12 + beat * 10);
      dx = hip + Math.sin(now * 4 + i * 0.3) * 3;
      dy = Math.abs(Math.sin(now * 5)) * -(6 + beat * 6) + beat * 6;
      rot = Math.sin(now * 2.5) * 0.15;
      scale = 1 + beat * 0.12;
    } else if (genre === 'orchestra') {
      var swing = Math.sin(now * 1.2 + ratio * Math.PI);
      dy = swing * (10 + beat * 16);
      dx = Math.cos(now * 0.8 + i * 0.15) * (2 + beat * 4);
      rot = Math.sin(now * 0.6) * 0.04;
      scale = 1 + Math.abs(swing) * 0.04 + beat * 0.1;
    } else if (genre === 'jazz') {
      var jswing = Math.sin(now * Math.PI * 1.8 + i * 0.3);
      var shuffle = ((now * 2) % 1) < 0.67 ? 1 : 0.5;
      dx = jswing * (6 + beat * 6) * shuffle;
      dy = Math.cos(now * 2.4 + i * 0.4) * (8 + beat * 6) * shuffle;
      rot = jswing * 0.06;
      scale = 1 + beat * 0.09;
    } else if (genre === 'ethnic') {
      var phase = now * 1.8 + i * 0.35;
      dx = Math.cos(phase) * (5 + beat * 6);
      dy = Math.sin(phase * 2) * (4 + beat * 4) + Math.abs(Math.sin(now * 3)) * (4 + beat * 8);
      rot = Math.sin(phase) * 0.05;
      scale = 1 + beat * 0.1;
    } else if (genre === 'freestyle') {
      var r1 = Math.sin(now * (2 + i * 0.2) + i);
      var r2 = Math.cos(now * (3 + i * 0.15));
      dx = r1 * (10 + beat * 10);
      dy = r2 * (12 + beat * 12);
      rot = r1 * r2 * 0.15;
      scale = 1 + Math.abs(r1 * r2) * 0.1 + beat * 0.1;
    }
    return { dx: dx, dy: dy, rot: rot, scale: scale };
  }

  function renderDanceFrame(genre, now, energy, fade){
    ctx.clearRect(0, 0, CW, CH);
    var simulatedBeat = 0.5 + 0.5 * Math.sin(now * 2 * Math.PI * 2);
    var beat = energy > 0.05 ? (energy * 0.85 + simulatedBeat * 0.15) : simulatedBeat * 0.35;
    var fs = calcFontSize(CW, CH);
    ctx.font = '900 ' + fs + "px 'Segoe UI',system-ui,sans-serif";
    ctx.textAlign = 'left'; ctx.textBaseline = 'middle';
    var textW = ctx.measureText(TEXT).width;
    var leftPad = 8, baseY = CH / 2;
    var colors = DANCE_GENRE_COLORS[genre] || DANCE_GENRE_COLORS.freestyle;
    var gradShift = (now * (30 + energy * 40)) % textW;
    var grad = ctx.createLinearGradient(leftPad - gradShift, 0, leftPad - gradShift + textW, 0);
    colors.forEach(function(c, i){ grad.addColorStop(i / (colors.length - 1), c); });

    ctx.save();
    ctx.globalAlpha = fade;
    ctx.shadowColor = colors[0];
    ctx.shadowBlur = 18 + beat * 22;

    var curX = leftPad;
    TEXT.split('').forEach(function(ch, i){
      var chW = ctx.measureText(ch).width;
      var ratio = (curX + chW / 2 - leftPad) / textW;
      var m = danceMotionForChar(genre, now, i, ratio, beat);
      ctx.save();
      ctx.translate(curX + chW / 2 + m.dx, baseY + m.dy);
      ctx.rotate(m.rot);
      ctx.scale(m.scale, m.scale);
      ctx.fillStyle = grad;
      ctx.textAlign = 'center';
      ctx.fillText(ch, 0, 0);
      ctx.restore();
      curX += chW;
    });
    ctx.restore();

    ctx.save();
    ctx.globalAlpha = fade * 0.75;
    ctx.font = '700 ' + Math.max(14, fs * 0.18) + "px 'Segoe UI',system-ui,sans-serif";
    ctx.textAlign = 'left'; ctx.textBaseline = 'top';
    ctx.fillStyle = colors[0];
    ctx.shadowColor = 'rgba(0,0,0,0.5)'; ctx.shadowBlur = 8;
    ctx.fillText(DANCE_GENRE_NAMES[genre] || genre, 12, 10);
    ctx.restore();
  }

  var particles = [];
  var exploding = false, explodeTimer = 0;
  var EXPLODE_HOLD = 90, REFORM_SPEED = 0.025;

  function getTextPixels(w, h){
    var scale = Math.min(window.devicePixelRatio || 1, 4);
    var ow = Math.round(w * scale), oh = Math.round(h * scale);
    var off = document.createElement('canvas');
    off.width = ow; off.height = oh;
    var ox = off.getContext('2d');
    var fs = calcFontSize(w, h) * scale;
    ox.font = '900 ' + fs + "px 'Segoe UI',system-ui,sans-serif";
    ox.fillStyle = '#fff'; ox.textAlign = 'left'; ox.textBaseline = 'middle';
    ox.fillText(TEXT, 8 * scale, oh / 2);
    var d;
    try { d = ox.getImageData(0, 0, ow, oh).data; } catch (e) { return []; }
    var pts = [];
    var step = Math.max(2, Math.round(scale * 2.5));
    for (var y = 0; y < oh; y += step) {
      for (var x = 0; x < ow; x += step) {
        if (d[(y * ow + x) * 4 + 3] > 128) { pts.push({ x: x / scale, y: y / scale }); }
      }
    }
    return pts;
  }

  function initParticles(){
    var W = CW, H = CH;
    if (!W || !H) return;
    var pts = getTextPixels(W, H);
    if (!pts.length) return;
    particles = pts.map(function(p){
      return {
        tx: p.x, ty: p.y, x: p.x, y: p.y, vx: 0, vy: 0,
        ex: (Math.random() - 0.5) * 18, ey: (Math.random() - 0.5) * 14 - 6,
        size: Math.random() * 1.6 + 1.0,
        phase: Math.random() * Math.PI * 2, phaseY: Math.random() * Math.PI * 2
      };
    });
    exploding = false; explodeTimer = 0;
  }

  function particleColor(p, now, W){
    var hue = (p.tx / W * 360 + now * GRAD_SPEED) % 360;
    return 'hsl(' + hue + ',100%,62%)';
  }

  function triggerExplode(){
    exploding = true; explodeTimer = 0;
    particles.forEach(function(p){
      var angle = Math.atan2(p.ty - CH / 2, p.tx - CW / 2);
      var speed = 8 + Math.random() * 16;
      p.ex = Math.cos(angle) * speed + (Math.random() - 0.5) * 10;
      p.ey = Math.sin(angle) * speed + (Math.random() - 0.5) * 10;
    });
  }

  var EFFECT_SEC = 5.0, REST_SEC = 10.0;
  var cycleStart = performance.now() / 1000;
  var cycleSubPhase = 'rest';

  function updateCycle(now, mode){
    var elapsed = now - cycleStart;
    if (cycleSubPhase === 'effect' && elapsed >= EFFECT_SEC) {
      cycleSubPhase = 'rest'; cycleStart = now;
    } else if (cycleSubPhase === 'rest' && elapsed >= REST_SEC) {
      cycleSubPhase = 'effect'; cycleStart = now;
      if (mode === 'explode') triggerExplode();
    }
  }

  function resetCycle(){
    cycleStart = performance.now() / 1000;
    cycleSubPhase = 'effect';
  }

  var logoMode = 'scroll';
  var danceFixedGenre = 'kabuki';
  var danceAiGenres = ['kabuki','egypt','india','hiphop','kpop','latin','orchestra','jazz','ethnic','freestyle'];
  var DANCE_AI_DURATION = 6;
  var danceAiStart = null;

  function animate(){
    requestAnimationFrame(animate);
    if (!CW || !CH) return;
    var energy = 0;
    var now = performance.now() / 1000;
    var mode = logoMode;

    if (mode === 'scroll') { drawTextGradient(now, null); return; }

    if (mode === 'dance') {
      var GENRES = ['kabuki','egypt','india','hiphop','kpop','latin'];
      var GENRE_DURATION = 8;
      if (animate._danceStart == null) animate._danceStart = now;
      var elapsed = now - animate._danceStart;
      var genreIdx = Math.floor(elapsed / GENRE_DURATION) % GENRES.length;
      var genre = GENRES[genreIdx];
      var phaseInGenre = (elapsed % GENRE_DURATION) / GENRE_DURATION;
      var fade = phaseInGenre < 0.15 ? phaseInGenre / 0.15 : phaseInGenre > 0.85 ? (1 - phaseInGenre) / 0.15 : 1;
      renderDanceFrame(genre, now, energy, fade);
      return;
    }

    if (mode === 'danceFixed') { renderDanceFrame(danceFixedGenre, now, energy, 1); return; }

    if (mode === 'danceAI') {
      if (danceAiStart == null) danceAiStart = now;
      var aiElapsed = now - danceAiStart;
      var aiIdx = Math.floor(aiElapsed / DANCE_AI_DURATION) % danceAiGenres.length;
      var aiGenre = danceAiGenres[aiIdx];
      var aiPhase = (aiElapsed % DANCE_AI_DURATION) / DANCE_AI_DURATION;
      var aiFade = aiPhase < 0.1 ? aiPhase / 0.1 : aiPhase > 0.9 ? (1 - aiPhase) / 0.1 : 1;
      renderDanceFrame(aiGenre, now, energy, aiFade);
      return;
    }

    if (mode === 'wave') {
      ctx.clearRect(0, 0, CW, CH);
      var waveAmpX = 22 + energy * 28;
      var fs = calcFontSize(CW, CH);
      ctx.font = '900 ' + fs + "px 'Segoe UI',system-ui,sans-serif";
      ctx.textAlign = 'left'; ctx.textBaseline = 'middle';
      var textW = ctx.measureText(TEXT).width;
      var leftPad = 8, baseY = CH * 0.68;
      var gradShift = (now * GRAD_SPEED / 360 * textW * 1.5) % textW;
      var grad = ctx.createLinearGradient(leftPad - gradShift, 0, leftPad - gradShift + textW * 1.5, 0);
      GRAD_STOPS.forEach(function(s){ grad.addColorStop(s[0], s[1]); });
      var curX = leftPad;
      TEXT.split('').forEach(function(ch){
        var chW = ctx.measureText(ch).width;
        var ratio = (curX + chW / 2 - leftPad) / textW;
        var dy = Math.sin(ratio * Math.PI * 2.5 - now * 2.8) * waveAmpX * 0.7
               + Math.sin(ratio * Math.PI * 4.5 + now * 1.9) * waveAmpX * 0.3;
        ctx.save();
        ctx.shadowColor = 'rgba(168,85,247,0.55)'; ctx.shadowBlur = 16;
        ctx.fillStyle = grad;
        ctx.fillText(ch, curX, baseY + dy);
        ctx.restore();
        curX += chW;
      });

      var ratio05 = 0.5;
      var v1 = -2.8 * Math.cos(ratio05 * Math.PI * 2.5 - now * 2.8) * waveAmpX * 0.7;
      var v2 = 1.9 * Math.cos(ratio05 * Math.PI * 4.5 + now * 1.9) * waveAmpX * 0.3;
      var netV = v1 + v2;
      var targetYaw = netV < 0 ? 75 : -75;
      shipYawDeg += (targetYaw - shipYawDeg) * 0.001;
      drawSailingShip(ctx, now, CW, CH, 1, shipYawDeg);
      return;
    }

    if (!particles.length) return;
    updateCycle(now, mode);
    var isResting = cycleSubPhase === 'rest';
    var restT = isResting ? Math.min(1, (now - cycleStart) / REST_SEC) : 0;
    var returnProgress = Math.min(1, restT / 0.4);
    var spring = isResting ? REFORM_SPEED + returnProgress * REFORM_SPEED * 2 : REFORM_SPEED;

    ctx.clearRect(0, 0, CW, CH);

    if (mode === 'explode') {
      if (isResting) {
        particles.forEach(function(p){
          p.vx = p.vx * 0.80 + (p.tx - p.x) * spring;
          p.vy = p.vy * 0.80 + (p.ty - p.y) * spring;
          p.x += p.vx; p.y += p.vy;
        });
      } else if (exploding) {
        explodeTimer++;
        particles.forEach(function(p){
          p.x += p.ex * (1 + energy * 2.5); p.y += p.ey * (1 + energy * 2.5);
          p.ex *= 0.91; p.ey *= 0.91; p.ey += 0.28;
        });
        if (explodeTimer > EXPLODE_HOLD) exploding = false;
      } else {
        particles.forEach(function(p){
          p.vx = p.vx * 0.80 + (p.tx - p.x) * REFORM_SPEED;
          p.vy = p.vy * 0.80 + (p.ty - p.y) * REFORM_SPEED;
          p.x += p.vx; p.y += p.vy;
        });
      }
    } else if (mode === 'orbit') {
      var orbitAmp = 1 + energy * 1.5, orbitSpeed = 1 + energy * 0.8;
      particles.forEach(function(p){
        var tx = p.tx, ty = p.ty;
        if (!isResting) {
          tx += 20 * orbitAmp * Math.cos(now * 1.6 * orbitSpeed + p.phase);
          ty += 14 * orbitAmp * Math.sin(now * 1.6 * orbitSpeed + p.phaseY);
        }
        var sp = isResting ? spring : 0.08, damp = 0.78;
        p.vx = p.vx * damp + (tx - p.x) * sp;
        p.vy = p.vy * damp + (ty - p.y) * sp;
        p.x += p.vx; p.y += p.vy;
      });
    }

    particles.forEach(function(p){
      var col = particleColor(p, now, CW);
      ctx.shadowColor = col; ctx.shadowBlur = isResting ? 3 : 7;
      ctx.fillStyle = col; ctx.globalAlpha = 0.93;
      ctx.beginPath(); ctx.arc(p.x, p.y, p.size, 0, Math.PI * 2); ctx.fill();
      ctx.globalAlpha = 1; ctx.shadowBlur = 0;
    });
  }

  window.addEventListener('resize', resize);
  setTimeout(function(){ resize(); animate(); }, 80);

  window.acSetLogoMode = function(mode){
    logoMode = mode;
    var btns = document.querySelectorAll('.logo-mode-btn');
    for (var i = 0; i < btns.length; i++) { btns[i].className = 'logo-mode-btn' + (btns[i].dataset.mode === mode ? ' active' : ''); }
    document.getElementById('danceFixedBar').className = mode === 'danceFixed' ? 'is-visible' : '';
    resetCycle();
    if (mode === 'explode') triggerExplode();
  };
  window.acSetLogoGenre = function(genre){
    danceFixedGenre = genre;
    var btns = document.querySelectorAll('.dance-fixed-btn');
    for (var i = 0; i < btns.length; i++) { btns[i].className = 'dance-fixed-btn' + (btns[i].dataset.genre === genre ? ' active' : ''); }
  };
})();
</script>"#;

/// 無料スマホ壁紙ダウンロードコーナーの実データ(`index.php` 1492〜1524行目、
/// `curl`で実データ確認済みの実画像URL、`o0499108015778827097.png`等の
/// Ameba(stat.ameba.jp)ホスティング画像4件)。画像・ダウンロードリンクとも
/// PHP版と同一の実URLをそのまま使用する(プレースホルダではない)。
const TOP_WALLPAPERS: &[(&str, &str, &str)] = &[
    // (画像URL, alt/表示名, download属性のファイル名)
    (
        "https://stat.ameba.jp/user_images/20260505/16/www-aon/87/8b/p/o0499108015778827097.png",
        "NTT IOWN 井上飛鳥さん 黒服",
        "ino-asuka-black.png",
    ),
    (
        "https://stat.ameba.jp/user_images/20260505/16/www-aon/1e/87/j/o0499108015778824305.jpg",
        "NTT IOWN 井上飛鳥さん 白服",
        "ino-asuka-white.jpg",
    ),
    (
        "https://stat.ameba.jp/user_images/20260505/16/www-aon/c2/90/p/o0499108015778820256.png",
        "大阪の神様ビリケン",
        "osaka-billiken.png",
    ),
    (
        "https://stat.ameba.jp/user_images/20260505/18/www-aon/16/38/p/o0498108015778869116.png",
        "大型スピーカー紹介のTONOさん",
        "tono-speaker.png",
    ),
];

/// PHP版がYouTube背景プレイヤーの初期状態(未クリック時のフォールバック)として
/// 実際に読み込む動画ID(`index.php` 3272行目 `DEFAULT_BG_VIDEO_ID='mSDVnO5gFYk'`、
/// `instantiateYtBgPlayer(DEFAULT_BG_VIDEO_ID, 0)`の呼び出し元複数箇所で実際に
/// 使用されている)。本ポートではPHP側のような検索駆動の動画切り替え
/// (`fetchAndCollect`等、クライアント側の巨大なJSロジック)は移植せず
/// (クライアントJSを持たないこのRustサイトのアーキテクチャ方針)、実際に
/// ページ初期表示で使われる本物の埋め込みIDをそのまま`<iframe>`で埋め込む。
const TOP_DEFAULT_BG_VIDEO_ID: &str = "mSDVnO5gFYk";

/// 言語コード(`g`フィールド)から、PHP版`makeAcNavAudiocafeRoot()`
/// (`index.php` 1786〜1789行目)と同じ式でGoogle翻訳プロキシURLを組み立てる。
/// 日本語のみ翻訳なしの素のURL。
fn google_translate_proxy_url(target_url: &str, gc: &str) -> String {
    if gc == "ja" {
        target_url.to_string()
    } else {
        format!(
            "https://translate.google.com/translate?sl=ja&tl={}&u={}",
            percent_encode(gc),
            percent_encode(target_url)
        )
    }
}

/// PHP版`makeAcNavGoogleTransUrl()`(`index.php` 1835〜1844行目)と同じ
/// tld割り当て表・パラメータ構成でGoogle翻訳サイト自体のURL(テキスト翻訳UI)を
/// 組み立てる(「🌐 Google翻訳サイトへ移動」ボタン相当)。
fn google_translate_site_url(gc: &str, page_url: &str) -> String {
    let (tl, hl) = if gc == "ja" { ("en", "ja") } else { (gc, gc) };
    let tld = match hl {
        "zh-CN" => "com.cn",
        "zh-TW" => "com.tw",
        "ru" => "ru",
        "de" => "de",
        "fr" => "fr",
        "it" => "it",
        "es" => "es",
        "pt" => "com.br",
        "ko" => "co.kr",
        "ja" => "co.jp",
        _ => "com",
    };
    format!(
        "https://translate.google.{}/?hl={}&sl=ja&tl={}&text={}&op=translate",
        tld,
        percent_encode(hl),
        percent_encode(tl),
        percent_encode(page_url)
    )
}

/// エッセイ本文(`c`/`d`フィールド)を段落表示用HTMLに変換する。PHP版JSでは
/// プレーンテキストのまま`textContent`風に描画していたが、Rust版はSSRのため
/// 空行(`\n\n`)を`<p>`区切り、単独改行(`\n`)を`<br>`に変換して読みやすくする
/// (内容自体は無加工、HTMLエスケープのみ実施)。
/// エッセイ本文(`c`/`d`)中に埋め込まれた素のURL(`https://...`)を抽出する。
/// PHP版`extractHttpUrlsFromString()`(`mergeCardOutboundLinks`が使用、
/// `index.php` 1935行目)相当——`cardLinks`フィールドに載っていない
/// リンクも本文中に直接書かれていることがあるため、`/summary`画面の
/// リンク一覧を漏れなく揃えるために移植した。
fn extract_urls_from_text(text: &str) -> Vec<String> {
    static RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r#"https?://[^\s"'<>]+"#).unwrap());
    RE.find_iter(text)
        .map(|m| m.as_str().trim_end_matches(|c: char| ".,)]}\u{3002}\u{3001}".contains(c)).to_string())
        .collect()
}

fn essay_to_html(text: &str) -> String {
    text.split("\n\n")
        .map(|para| format!("<p>{}</p>", html_escape(para).replace('\n', "<br>")))
        .collect::<String>()
}

/// 言語カード1件を描画する。実際のPHP版が持つ全フィールド
/// (国旗・現地語表記・カードラベル・国名・地域に加え、`c`/`d`の全文エッセイ・
/// `cardLinks`・モーダル相当の遷移先リンク一式)をここで復元する
/// (2026-07-19、40件抜粋+短いフィールドのみ、という前回のスコープ縮小を解消)。
fn render_lang_card(card: &LangCard) -> String {
    // 英語(`c`)と日本語(`d`)の長文エッセイを見出しラベル付きで明確に分離する
    // (2026-07-19、ユーザー指示によりフォントサイズも二回り拡大——
    // `.card-essay`/`.card-essay-d`のCSS参照)。`d`が存在するカードは147件中
    // 10件のみ。
    let mut essay = format!(
        r#"<div class="card-essay"><span class="card-essay-label">English</span>{}"#,
        essay_to_html(&card.c)
    );
    if let Some(d) = &card.d {
        essay.push_str(&format!(
            r#"<div class="card-essay-d"><span class="card-essay-label">日本語</span>{}</div>"#,
            essay_to_html(d)
        ));
    }
    essay.push_str("</div>");

    let links = card
        .card_links
        .as_ref()
        .map(|links| {
            let items: String = links
                .iter()
                .map(|l| format!(r#"<li><a href="{}" target="_blank" rel="noopener noreferrer">{}</a></li>"#, html_escape(&l.href), html_escape(&l.label)))
                .collect();
            format!(r#"<ul class="card-links">{items}</ul>"#)
        })
        .unwrap_or_default();

    // PHP版の「言語カード選択後の遷移先を尋ねるモーダル」(`#acNavChoiceModal`、
    // `index.php` 1578〜1598行目)が提示する実際の遷移先(audiocafe.tokyo本体/
    // /aruaru/aruaru-lady/rakuten-mobile/Google翻訳サイト/aruaru.tokyo)を、
    // モーダルではなく直リンク行として復元する(このRustサイトはクライアント
    // 側JSを持たないアーキテクチャのため、(b)のプレーンHTML化を採用——
    // ユーザーはモーダルを経由せず1クリックで同じ行き先へ実際に到達できる)。
    let gc = &card.g;
    let ac_url = html_escape(&google_translate_proxy_url("https://audiocafe.tokyo/", gc));
    // /aruaru・/aruaru-lady・/rakuten-mobileへの遷移も、audiocafe.tokyo本体と
    // 同じくGoogle翻訳プロキシ経由にする(2026-07-19、ユーザー指摘により修正
    // ——従来は素の日本語ページへ直接リンクしており、選択した言語が
    // 反映されていなかった)。日本語(`gc=="ja"`)の場合は
    // `google_translate_proxy_url`が素のURLを返すため従来通り無翻訳。
    let aruaru_url = html_escape(&google_translate_proxy_url("https://audiocafe.tokyo/aruaru", gc));
    let aruaru_lady_url = html_escape(&google_translate_proxy_url("https://audiocafe.tokyo/aruaru-lady", gc));
    let rakuten_mobile_url = html_escape(&google_translate_proxy_url("https://audiocafe.tokyo/rakuten-mobile", gc));
    // PHP版「📋 言語カード＆/top 要約LIST」ボタン(`acNavBtnMixlist`、
    // `?mixlist=1`遷移、`index.php` 1590行目)の相当機能。全カードの
    // YouTube/Blog等のリンクをタイトル付きで整列表示する`/summary`画面へ、
    // このカードの言語での選択箇所(`#{gc}`)にアンカーして遷移する
    // (2026-07-19、ユーザー要望により新設)。
    let summary_url = html_escape(&google_translate_proxy_url(&format!("https://audiocafe.tokyo/summary#{gc}"), gc));
    let actions = format!(
        r#"<div class="card-actions">
<a href="{ac_url}" target="_blank" rel="noopener noreferrer">audiocafe.tokyo</a>
<a href="{aruaru_url}" target="_blank" rel="noopener noreferrer">/aruaru</a>
<a href="{aruaru_lady_url}" target="_blank" rel="noopener noreferrer">/aruaru-lady</a>
<a href="{rakuten_mobile_url}" target="_blank" rel="noopener noreferrer">/rakuten-mobile</a>
<a href="{aruaru_tokyo}" target="_blank" rel="noopener noreferrer">aruaru.tokyo</a>
<a href="{summary_url}" target="_blank" rel="noopener noreferrer">📋 言語カード要約</a>
<a href="{gt}" target="_blank" rel="noopener noreferrer">🌐 Google Translate</a>
</div>"#,
        aruaru_tokyo = html_escape(ARUARU_TOKYO_URL),
        gt = html_escape(&google_translate_site_url(gc, "https://audiocafe.tokyo/")),
    );

    // 遷移先リンク(`actions`)は、エッセイ本文(数百〜千文字を超える
    // カードもある)より前、国旗・ラベル直後に配置する。エッセイの後に
    // 置くと、長いカードではスクロールしないと遷移先リンクに到達できず、
    // 「国旗をクリックしても遷移先を選ぶ画面が出ない」ように見える実バグ
    // になっていたため(2026-07-19発見・修正)。PHP版のモーダルは
    // クリック直後に(本文を読まずとも)遷移先を選べる体験だったため、
    // このRust版でも同じ即時性を静的リンクで再現する。
    format!(
        r#"<div class="card"><a href="{ac_url}" target="_blank" rel="noopener noreferrer"><img class="card-flag" src="https://flagcdn.com/60x40/{fc}.png" alt="{label}"></a><span class="card-code">{label}</span><span class="card-native">{native}</span><span class="card-country">{name}</span>{actions}{essay}{links}</div>"#,
        fc = html_escape(&card.fc),
        label = html_escape(&card.a),
        native = html_escape(&card.t),
        name = html_escape(&card.n),
    )
}

/// PHP版トップページ(`index.php`、8150行)の実際の内容を移植した完全版
/// (2026-07-19、ユーザー指示により前回の40件抜粋+装飾機能除外という
/// スコープ縮小を解消し、フル移植へ拡張)。
///
/// **実際の内容(`curl https://audiocafe.tokyo/`で実データ確認済み)**:
/// `<title>AUDIOCAFE | World — Select Your Language</title>`を持つ、
/// 「147言語のうち好きな言語カードを選んでGoogle翻訳版へ進む」ための
/// 言語選択ランディングページ(ダークテーマ、`#000`背景・`#22d3ee`シアン
/// アクセント)。実際の構成は (1) ヘッダー(サブタイトル・日本語の注記・
/// 検索ボックス・地域絞り込みピル)、(2) 147件全件の言語カードグリッド
/// (国旗画像・現地語表記・英語名・国名・全文エッセイ・`cardLinks`・
/// 遷移先リンク一式)、(3) YouTube背景プレイヤー・無料スマホ壁紙
/// ダウンロードコーナー、(4) フッター(Copyright表記)。
///
/// **今回のスコープ内(完了)**: 147件全カード(前回40件から拡張)、各カードの
/// `c`(英語)/`d`(日本語、10件)の全文エッセイ本文(政治・宗教・地政学的な
/// 主張を含め、実際に公開済みのPHP版コンテンツをそのまま複製)、`cardLinks`
/// (8件のカードに存在、実際の`<a href>`として復元)、YouTube背景プレイヤー
/// (実際に使われるデフォルト動画ID`mSDVnO5gFYk`を`<iframe>`埋め込み)、
/// 無料スマホ壁紙コーナー(実画像4件、実ダウンロードリンク)、検索/地域
/// フィルタ(`?q=`/`?region=`のクエリパラメータによるサーバーサイド絞り込み、
/// クライアントJSを持たないこのサイトのアーキテクチャに合わせた実装)。
///
/// **今回のスコープ外(正直に開示)**: (1) PHP版の「クリックすると遷移先を
/// 尋ねるモーダル」(`#acNavChoiceModal`)自体はJSモーダルとして再現せず、
/// 各カードに直リンク行(`.card-actions`)として展開した(モーダルを経由
/// しないだけで、到達できる行き先自体はモーダルの選択肢と同一)。
/// (2) YouTube背景プレイヤーの検索駆動での動画切り替え(`fetchAndCollect`
/// 等、数千行のクライアントJS)は移植せず、初期表示の実際の動画のみ埋め込み。
/// (3) 壁紙コーナーの「タップで原寸表示」インタラクションはブラウザ標準の
/// 画像リンク遷移で代替(画像自体・ダウンロードリンクは実物)。
const SUMMARY_STYLE: &str = r#"<style>
.summary-page{max-width:52rem;margin:0 auto;padding:1.5rem 1rem 3rem}
.summary-page h1{font-size:1.6rem;margin-bottom:.3rem}
.summary-page .summary-note{color:var(--text-dim);font-size:.85rem;margin-bottom:1.5rem}
.summary-page .summary-card{border:1px solid var(--border);border-radius:.75rem;padding:1rem 1.2rem;margin-bottom:1rem;background:var(--surface);scroll-margin-top:1rem}
.summary-page .summary-card h2{font-size:1.1rem;margin:0 0 .5rem;color:var(--cyan)}
.summary-page .card-links{list-style:none;padding:0;margin:0;font-size:.9rem;line-height:1.7}
.summary-page .card-links li{margin-bottom:.2rem}
</style>"#;

/// PHP版「📋 言語カード＆/top 要約LIST」(`mixlist=1`、`index.php` 1590行目)の
/// 相当機能。全147言語カードそれぞれの`cardLinks`+本文中に埋め込まれたURLを
/// マージし(`extract_urls_from_text`、PHP版`mergeCardOutboundLinks`相当)、
/// カードごとにタイトル付きで整列表示する(2026-07-19新設)。PHP版はさらに
/// Google翻訳ウィジェットで選択言語に画面全体を翻訳する仕組みだったが、
/// このRust版では各カードの`.card-actions`からこの画面へGoogle翻訳
/// プロキシ経由でアンカー付き遷移することで同じ体験を実現している
/// (このページ自体は日本語+各リンクの原文表示のみ、スコープ縮小として
/// 正直に開示)。
fn render_summary_body() -> String {
    use std::collections::HashSet;
    let mut sections = String::new();
    for card in TOP_LANGUAGES.iter() {
        let mut seen: HashSet<String> = HashSet::new();
        let mut links: Vec<(String, String)> = Vec::new();
        if let Some(card_links) = &card.card_links {
            for l in card_links {
                if seen.insert(l.href.clone()) {
                    links.push((l.href.clone(), l.label.clone()));
                }
            }
        }
        for url in extract_urls_from_text(&card.c).into_iter().chain(extract_urls_from_text(card.d.as_deref().unwrap_or(""))) {
            if seen.insert(url.clone()) {
                links.push((url.clone(), url.clone()));
            }
        }
        if links.is_empty() {
            continue;
        }
        let items: String = links
            .iter()
            .map(|(href, label)| format!(r#"<li><a href="{}" target="_blank" rel="noopener noreferrer">{}</a></li>"#, html_escape(href), html_escape(label)))
            .collect();
        sections.push_str(&format!(
            r#"<section class="summary-card" id="{gc}"><h2>{flag} {name}（{native}）</h2><ul class="card-links">{items}</ul></section>"#,
            gc = html_escape(&card.g),
            flag = format!(r#"<img src="https://flagcdn.com/24x16/{}.png" alt="" width="24" height="16" style="vertical-align:middle;border-radius:2px">"#, html_escape(&card.fc)),
            name = html_escape(&card.n),
            native = html_escape(&card.t),
        ));
    }
    format!(
        r##"{SUMMARY_STYLE}
<div class="summary-page">
<h1>📋 言語カード要約 — Link Summary</h1>
<p class="summary-note">147言語カードそれぞれが持つYouTube・ブログ等へのリンクをタイトル付きで一覧表示しています。各カードの「📋 言語カード要約」リンクから、その言語のセクションへ直接ジャンプできます。<a href="/#lang-grid">← 言語カード一覧へ戻る</a></p>
{sections}
</div>"##
    )
}

fn render_top_body(query: &std::collections::HashMap<String, String>) -> String {
    // PHP版`REGIONS`配列(`index.php` 1762行目)と同じ並び順・
    // `REGION_LABELS`(1763〜1770行目)と同じ英語見出しを踏襲、日本語は
    // 括弧書きで併記する(2026-07-19、ユーザー指示によりデフォルトを
    // 英語+(日本語)表記に変更)。
    let region_order = ["Asia", "Europe", "Americas", "Middle East", "Africa", "Pacific"];
    let region_label = |r: &str| -> &'static str {
        match r {
            "Asia" => "Asia (アジア)",
            "Europe" => "Europe (ヨーロッパ)",
            "Americas" => "Americas (南北アメリカ)",
            "Middle East" => "Middle East & Central Asia (中東・中央アジア)",
            "Africa" => "Africa (アフリカ)",
            "Pacific" => "Pacific (太平洋)",
            _ => "Other (その他)",
        }
    };

    let q = query.get("q").map(|s| s.to_lowercase()).unwrap_or_default();
    let region_filter = query.get("region").filter(|s| !s.is_empty());

    let matches = |card: &LangCard| -> bool {
        if let Some(rf) = region_filter {
            if card.r != *rf {
                return false;
            }
        }
        if q.is_empty() {
            return true;
        }
        card.n.to_lowercase().contains(&q) || card.t.to_lowercase().contains(&q) || card.a.to_lowercase().contains(&q)
    };

    // PHP版`render()`のロジックに合わせる(`index.php` 1992〜2047行目):
    // 地域フィルタ未指定(`activeRegion==="all"`)の場合はL配列の元の並び順の
    // ままフラットに1セクションで表示し、地域ごとのグループ化は行わない
    // (グループ化は特定の地域ピルを選択した時のみ、`region-title`見出し付きで
    // 表示する)。旧実装は常に6地域へグループ化していたため、デフォルト表示の
    // 先頭が「イラン→日本→…」というPHP版の実際の並び順と異なっていた
    // (2026-07-19、ユーザー指摘により修正)。
    let matched: Vec<&LangCard> = TOP_LANGUAGES.iter().filter(|c| matches(c)).collect();
    let shown = matched.len();
    let sections = if shown == 0 {
        r#"<p class="note">条件に一致する言語カードが見つかりません。</p>"#.to_string()
    } else if let Some(rf) = region_filter {
        let cards: String = matched.iter().map(|c| render_lang_card(c)).collect();
        format!(
            r#"<section class="region-section"><h2 class="region-title">{}</h2><div class="grid">{cards}</div></section>"#,
            region_label(rf)
        )
    } else {
        let cards: String = matched.iter().map(|c| render_lang_card(c)).collect();
        format!(r#"<section class="region-section"><div class="grid">{cards}</div></section>"#)
    };

    // 地域絞り込みピル(PHP版`#pills`相当、JSでのDOM生成をサーバーサイドの
    // クエリパラメータリンクへ置き換え——クリックのみで動作し、JS不要)。
    let mut pills = format!(
        r#"<a class="pill{}" href="/{}">All ({})</a>"#,
        if region_filter.is_none() { " is-active" } else { "" },
        if q.is_empty() { String::new() } else { format!("?q={}", percent_encode(query.get("q").unwrap())) },
        TOP_LANGUAGES.len(),
    );
    for region in region_order {
        let count = TOP_LANGUAGES.iter().filter(|c| c.r == region).count();
        if count == 0 {
            continue;
        }
        let is_active = region_filter.map(|r| r == region).unwrap_or(false);
        let qs = if q.is_empty() {
            format!("?region={}", percent_encode(region))
        } else {
            format!("?region={}&q={}", percent_encode(region), percent_encode(query.get("q").unwrap()))
        };
        pills.push_str(&format!(
            r#"<a class="pill{}" href="/{qs}">{} ({count})</a>"#,
            if is_active { " is-active" } else { "" },
            region_label(region),
        ));
    }

    let q_value = query.get("q").map(|s| html_escape(s)).unwrap_or_default();
    let region_hidden = region_filter
        .map(|r| format!(r#"<input type="hidden" name="region" value="{}">"#, html_escape(r)))
        .unwrap_or_default();

    let wallpapers: String = TOP_WALLPAPERS
        .iter()
        .map(|(url, name, filename)| {
            format!(
                r#"<div class="yt-wp-item"><a href="{url}" target="_blank" rel="noopener noreferrer"><img class="yt-wp-thumb" loading="lazy" src="{url}?caw=200" alt="{name} スマホ壁紙"></a><span class="yt-wp-name">{name}</span><a class="yt-wp-dl" href="{url}" download="{filename}" target="_blank" rel="noopener noreferrer">⬇ 保存</a></div>"#,
                url = html_escape(url),
                name = html_escape(name),
                filename = html_escape(filename),
            )
        })
        .collect();

    // YouTube再生リストのシリーズ機能(PHP版`SEARCH_SERIES`、77件)の復活。
    // 各シリーズの`urls`から再生可能な動画ID(`scraper::extract_yt_id`、
    // `/discover`のサムネイル抽出と同じロジックを再利用)を抽出し、1件も
    // 再生可能な動画が無いシリーズ(`/results?search_query=...`のみ等)は、
    // PHP版の既存方針(`audiocafe.tokyo/CLAUDE.md`——YouTube検索結果の
    // スクレイプ推測再生はしない)を踏襲し、実際のYouTube検索結果ページへの
    // 直接遷移(新規タブ)にフォールバックする。
    let series_payload: Vec<serde_json::Value> = SEARCH_SERIES
        .iter()
        .map(|s| {
            let ids: Vec<String> = s.urls.iter().filter_map(|u| scraper::extract_yt_id(u)).collect();
            let label = if s.label.is_empty() { s.btn.clone() } else { s.label.clone() };
            serde_json::json!({
                "label": label,
                "ids": ids,
                "searchUrl": format!("https://www.youtube.com/results?search_query={}", percent_encode(&s.btn)),
            })
        })
        .collect();
    let series_json = serde_json::to_string(&series_payload).unwrap_or_else(|_| "[]".to_string());
    let series_buttons: String = SEARCH_SERIES
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let label = if s.label.is_empty() { &s.btn } else { &s.label };
            format!(
                r#"<button type="button" class="yt-series-btn{}" data-idx="{i}" onclick="acPlaySeries({i})">{}</button>"#,
                if i == 0 { " is-active" } else { "" },
                html_escape(label),
            )
        })
        .collect();
    // PHP版コメント(`index.php` 195〜197行目・4625〜4633行目)の通り、
    // 起動時はSEARCH_SERIES[0]の先頭URLを再生する(`TOP_DEFAULT_BG_VIDEO_ID`は
    // PHP側でも「再生候補が全て失敗した場合のみ使用」するフォールバック専用
    // 定数であり、実際の初期表示動画ではなかった——2026-07-19訂正)。
    let default_series_label = SEARCH_SERIES
        .first()
        .map(|s| if s.label.is_empty() { s.btn.clone() } else { s.label.clone() })
        .unwrap_or_default();
    let default_id = SEARCH_SERIES
        .first()
        .and_then(|s| s.urls.iter().find_map(|u| scraper::extract_yt_id(u)))
        .unwrap_or_else(|| TOP_DEFAULT_BG_VIDEO_ID.to_string());
    let default_series_label_esc = html_escape(&default_series_label);
    // PHP版`#ytNowPlaying`(`index.php` 1459〜1463行目、「タップ→動画/
    // ページを開く」)相当。「今再生中」表示をクリック可能なリンクにし、
    // 実際のYouTube視聴ページ(`watch?v=`)へ遷移できるようにする
    // (2026-07-19、ユーザー指摘により復活——旧実装は非クリック可能な
    // ただのテキストだった)。
    let default_now_url = html_escape(&format!("https://www.youtube.com/watch?v={default_id}"));
    let blog_title_ja_esc = html_escape(BLOG_POST_TITLE_JA);
    let blog_title_en_esc = html_escape(BLOG_POST_TITLE_EN);

    let list: String = RANKINGS
        .iter()
        .map(|(slug, _, label)| format!(r#"<li><a href="/ranking/{slug}">{}</a></li>"#, html_escape(label)))
        .collect();
    let composite_list: String = COMPOSITE_PAGES
        .iter()
        .map(|p| format!(r#"<li><a href="/page/{}">{}</a></li>"#, p.slug, html_escape(p.title)))
        .collect();

    format!(
        r##"{TOP_STYLE}
<div class="top-page">
<div class="header">
<span class="logo">audiocafe.tokyo</span>
<p class="subtitle">Please select your native language.</p>
<p class="note">あなたの母国語を選択してください。2回目の選択時はブラウザを閉じて再度開いてください。<br>動画を視聴するには日本語を選択してください。</p>
<p class="lang-select-link"><a href="#lang-grid">🌐 Select your language / 言語を選択する（世界中の言語から選べます） →</a></p>
<p class="blog-link"><a href="{BLOG_POST_URL}" target="_blank" rel="noopener noreferrer">📝 {blog_title_ja_esc}</a> / <a href="{BLOG_POST_URL}" target="_blank" rel="noopener noreferrer">{blog_title_en_esc}</a></p>
<div class="yt-bg-player" id="ytBgPlayer">
<button type="button" class="yt-panel-close" id="ytPanelClose" onclick="acToggleYtPanel(false)">✕ CLOSE</button>
<div class="yt-now-playing">
<a id="ytNowPlaying" href="{default_now_url}" target="_blank" rel="noopener noreferrer" title="タップ → 本物のYouTubeで最初から再生">▶ <span id="ytNowTitle">{default_series_label_esc}</span></a>
<a id="ytNowUrl" href="{default_now_url}" target="_blank" rel="noopener noreferrer" title="タップ → 本物のYouTubeで最初から再生">{default_now_url}</a>
</div>
<iframe id="ytBgIframe" width="100%" height="220" src="https://www.youtube.com/embed/{default_id}?autoplay=1&mute=1&rel=0" title="AUDIOCAFE background video" frameborder="0" allow="autoplay; encrypted-media" allowfullscreen loading="lazy"></iframe>
<div class="yt-series-controls"><button type="button" class="yt-next-btn" onclick="acNextVideo()">⏭ NEXT</button></div>
<div class="yt-series-list">{series_buttons}</div>
</div>
<button type="button" class="yt-panel-open" id="ytPanelOpen" style="display:none" onclick="acToggleYtPanel(true)">▶ OPEN</button>
<script type="application/json" id="ytSeriesData">{series_json}</script>
<script>
(function(){{
  var data = JSON.parse(document.getElementById('ytSeriesData').textContent);
  var cur = 0, idx = 0;
  var iframe = document.getElementById('ytBgIframe');
  var nowPlaying = document.getElementById('ytNowPlaying');
  var nowTitle = document.getElementById('ytNowTitle');
  var nowUrl = document.getElementById('ytNowUrl');
  function embedUrl(id) {{ return 'https://www.youtube.com/embed/' + id + '?autoplay=1&mute=1&rel=0'; }}
  function watchUrl(id) {{ return 'https://www.youtube.com/watch?v=' + id; }}
  function updateActive() {{
    var btns = document.querySelectorAll('.yt-series-btn');
    for (var i = 0; i < btns.length; i++) {{ btns[i].className = 'yt-series-btn' + (i === cur ? ' is-active' : ''); }}
  }}
  // 動画の上のタイトル+URL表示(PHP版`#ytNowPlaying`「タップ→動画/
  // ページを開く」相当)。タイトル・URLのどちらをタップしても、実際の
  // YouTube視聴ページ(タイムスタンプ無し=常に最初から再生)へ新規タブで
  // 遷移する(2026-07-19、ユーザー指摘により動画の上に移動+URL表示も
  // クリック可能化)。
  function setNowPlaying(url, label) {{
    nowPlaying.href = url; nowUrl.href = url;
    nowTitle.textContent = label; nowUrl.textContent = url;
  }}
  window.acPlaySeries = function(i) {{
    var s = data[i];
    if (!s) return;
    cur = i; idx = 0;
    if (s.ids.length > 0) {{
      iframe.src = embedUrl(s.ids[0]);
      setNowPlaying(watchUrl(s.ids[0]), s.label);
    }} else {{
      window.open(s.searchUrl, '_blank', 'noopener');
      setNowPlaying(s.searchUrl, s.label + '（YouTube検索結果へ）');
    }}
    updateActive();
  }};
  window.acNextVideo = function() {{
    var s = data[cur];
    if (!s) return;
    if (s.ids.length === 0) {{ window.open(s.searchUrl, '_blank', 'noopener'); return; }}
    idx = (idx + 1) % s.ids.length;
    iframe.src = embedUrl(s.ids[idx]);
    setNowPlaying(watchUrl(s.ids[idx]), s.label);
  }};
  window.acToggleYtPanel = function(open) {{
    document.getElementById('ytBgPlayer').style.display = open ? '' : 'none';
    document.getElementById('ytPanelOpen').style.display = open ? 'none' : 'block';
  }};
}})();
</script>
<!-- PHP版のcanvasパーティクルロゴ+YouTube音楽連動(`index.php` 5854行目以降、
     1000行超)の移植。文字の描画式(`danceMotionForChar`の10ジャンル分の
     dx/dy/rot/scale計算式、波モードの二重sin波、爆発/オービットの
     パーティクル物理)はPHP版のものをそのままJSへ移植し、実際にcanvas上で
     文字が動く——CSSキーフレームでの簡略化(旧版)をやめた(2026-07-19、
     ユーザー指摘により再実装)。省略したのはYouTube音量連動(`ytEnergy`、
     このRust版はYouTube Player APIと接続していないため常に0扱い=PHP版の
     「無音時のBPM模擬フォールバック」相当の動きになる)と、波モード専用の
     帆船装飾(`drawSailingShip`、ロゴ本体の動きとは別の付随演出)、Dance AI
     モードの音量パターン解析(`detectGenreFromEnergy`、実音声エネルギー
     入力が無いため10ジャンルを一定間隔でローテーションする方式に簡略化)の
     3点のみ——正直に開示。 -->
<div id="logoWrap"><canvas id="acLogoCanvas"></canvas></div>
<div id="logoModeBar">
<button type="button" class="logo-mode-btn active" data-mode="scroll" onclick="acSetLogoMode('scroll')">🌈 スクロール</button>
<button type="button" class="logo-mode-btn" data-mode="dance" onclick="acSetLogoMode('dance')">💃 Dance</button>
<button type="button" class="logo-mode-btn" data-mode="danceFixed" onclick="acSetLogoMode('danceFixed')">🎯 Dance Fixed</button>
<button type="button" class="logo-mode-btn" data-mode="danceAI" onclick="acSetLogoMode('danceAI')">🤖 Dance AI</button>
<button type="button" class="logo-mode-btn" data-mode="wave" onclick="acSetLogoMode('wave')">🌊 波</button>
<button type="button" class="logo-mode-btn" data-mode="explode" onclick="acSetLogoMode('explode')">💥 爆発</button>
<button type="button" class="logo-mode-btn" data-mode="orbit" onclick="acSetLogoMode('orbit')">🌀 オービット</button>
</div>
<div id="danceFixedBar">
<span class="df-label">Genre →</span>
<button type="button" class="dance-fixed-btn active" data-genre="kabuki" onclick="acSetLogoGenre('kabuki')">🎭 歌舞伎</button>
<button type="button" class="dance-fixed-btn" data-genre="egypt" onclick="acSetLogoGenre('egypt')">🏛 エジプト</button>
<button type="button" class="dance-fixed-btn" data-genre="india" onclick="acSetLogoGenre('india')">🕉 インド</button>
<button type="button" class="dance-fixed-btn" data-genre="hiphop" onclick="acSetLogoGenre('hiphop')">🎤 HIPHOP</button>
<button type="button" class="dance-fixed-btn" data-genre="kpop" onclick="acSetLogoGenre('kpop')">💖 K-POP</button>
<button type="button" class="dance-fixed-btn" data-genre="latin" onclick="acSetLogoGenre('latin')">💃 ラテン</button>
<button type="button" class="dance-fixed-btn" data-genre="orchestra" onclick="acSetLogoGenre('orchestra')">🎻 オーケストラ</button>
<button type="button" class="dance-fixed-btn" data-genre="jazz" onclick="acSetLogoGenre('jazz')">🎷 JAZZ</button>
<button type="button" class="dance-fixed-btn" data-genre="ethnic" onclick="acSetLogoGenre('ethnic')">🪘 民族</button>
<button type="button" class="dance-fixed-btn" data-genre="freestyle" onclick="acSetLogoGenre('freestyle')">✨ フリースタイル</button>
</div>
{LOGO_CANVAS_SCRIPT}
<div class="yt-wp-corner">
<span class="yt-wp-head">🎁 無料 スマホ壁紙コーナー</span>
<span class="yt-wp-hint">画像をタップで原寸表示 →「⬇ 保存」または画像長押しで端末に保存できます。</span>
<div class="yt-wp-grid">{wallpapers}</div>
</div>
<form class="search-wrap" method="get" action="/">
<input type="text" name="q" value="{q_value}" placeholder="Search languages...">
{region_hidden}
<button type="submit">Search</button>
</form>
<div class="pills">{pills}</div>
</div>
<div class="main" id="lang-grid">
{sections}
<div class="nav-box">
<h2>総合ページ(既存PHP側の/aruaru・/aruaru-lady・/rakuten-mobileに相当)</h2>
<ul>{composite_list}</ul>
</div>
<div class="nav-box">
<h2>個別ランキング(Rust版独自の内部ナビゲーション)</h2>
<ul>{list}</ul>
</div>
</div>
<div class="footer">
<p>Copyright &copy; 2025 <a href="/">audiocafe.tokyo</a>, Akiru Akiruno-City Tokyo Japan. All Rights Reserved.</p>
<p>Powered by Google Translate</p>
</div>
</div>
"##
    )
}

async fn ranking_page(params: Params) -> hyper_compat::Response {
    let slug = params.get("slug").unwrap_or("");
    let Some((_, filename, label)) = RANKINGS.iter().find(|(s, _, _)| *s == slug) else {
        return hyper_compat::html_response(
            StatusCode::NOT_FOUND,
            page_shell("見つかりません", "<h1>404</h1><p>未対応のランキングです。</p>"),
        );
    };
    match fetch_cache(filename).await {
        Ok(data) => hyper_compat::html_response(StatusCode::OK, page_shell(label, &render_ranking_body(label, &data))),
        Err(e) => hyper_compat::html_response(
            StatusCode::OK,
            page_shell("エラー", &format!("<h1>取得エラー</h1><p>{}</p>", html_escape(&e))),
        ),
    }
}

async fn composite_page_by_slug(slug: &str) -> hyper_compat::Response {
    // `/rakuten-mobile`はPHP版の専用ページ(917行、独自の見出し・マーケティング
    // 文言・カード構成を持つ)であり、他の複合ページと違い汎用JSONダンプ
    // (`render_value_generic`)では全く別のページになってしまうことが
    // 2026-07-19の監査で判明した。そのためこのslugだけは専用の
    // `render_rakuten_mobile_body`でPHP版の実際の内容を再現する
    // (データ取得自体は既存の`fetch_cache`アーキテクチャのまま)。
    if slug == "rakuten-mobile" {
        return hyper_compat::html_response(
            StatusCode::OK,
            page_shell("📶 楽天モバイル情報 — audiocafe.tokyo", &render_rakuten_mobile_body().await),
        );
    }
    // `/aruaru-lady`もPHP版が独自の見出し・マーケティング文言・CSS装飾
    // (ダークテーマ、`.hero`/`.card`/`.toc`等)を持つ専用ページであり、
    // 汎用JSONダンプでは全く別のページになってしまう(2026-07-19監査、
    // `/rakuten-mobile`と同種の問題)。専用レンダラーで再現する。
    if slug == "aruaru-lady" {
        return hyper_compat::html_response(
            StatusCode::OK,
            page_shell("💃 女性向けお仕事情報｜キャバレー・キャバクラ・TVチャットレディ | audiocafe.tokyo", &render_aruaru_lady_body().await),
        );
    }
    // `/aruaru`もPHP版が独自の見出し・マーケティング文言・CSS装飾
    // (ダークテーマ、doda求人ピックアップ・技術TOP80ランキング・学習サービス
    // TOP50・英会話TOP50等)を持つ専用ページであり、汎用JSONダンプ
    // (旧`COMPOSITE_PAGES`の`aruaru`エントリはキャバクラ/熟女キャバランキング
    // を列挙していたが、これらは実際には`/aruaru-lady`へ移転済みで
    // `/aruaru`側にはもう無い)では全く別のページになってしまう
    // (2026-07-19監査、`/rakuten-mobile`・`/aruaru-lady`と同種の問題)。
    // 専用レンダラーで再現する。
    if slug == "aruaru" {
        return hyper_compat::html_response(
            StatusCode::OK,
            page_shell("aruaru | ITエンジニア案件・求人マッチング | audiocafe.tokyo", &render_aruaru_body().await),
        );
    }
    let Some(page) = COMPOSITE_PAGES.iter().find(|p| p.slug == slug) else {
        return hyper_compat::html_response(
            StatusCode::NOT_FOUND,
            page_shell("見つかりません", "<h1>404</h1><p>未対応のページです。</p>"),
        );
    };
    hyper_compat::html_response(StatusCode::OK, page_shell(page.title, &render_composite_body(page).await))
}

async fn composite_page(params: Params) -> hyper_compat::Response {
    composite_page_by_slug(params.get("slug").unwrap_or("")).await
}

/// PHP側の`build_lists($SEED_URLS)`(テキストリンク・動画リンク・写真の
/// 収集アルゴリズム)を移植した`scraper::build_lists`を呼び出して表示する。
/// 元のPHPはトップページ内で条件付きに埋め込んでいたが、Rust側では
/// 独立したページ`/discover`として切り出した。
async fn discover_page() -> hyper_compat::Response {
    let lists = scraper::build_lists(seed_urls::SEED_URLS).await;

    let video_items: String = lists
        .video_links
        .iter()
        .map(|v| {
            let thumb = scraper::extract_yt_id(&v.url)
                .map(|id| format!(r#"<img src="https://i.ytimg.com/vi/{id}/default.jpg" alt="" style="vertical-align:middle;margin-right:0.5rem;">"#))
                .unwrap_or_default();
            format!(
                r#"<li><a href="{}" target="_blank" rel="noopener noreferrer">{}▶️ [{}] {}</a></li>"#,
                html_escape(&v.url),
                thumb,
                html_escape(scraper::source_name(&v.url)),
                html_escape(&v.title)
            )
        })
        .collect();

    let text_items: String = lists
        .text_links
        .iter()
        .map(|t| format!(r#"<li><a href="{}" target="_blank" rel="noopener noreferrer">{}</a></li>"#, html_escape(&t.url), html_escape(&t.title)))
        .collect();

    let photo_items: String = lists
        .photos
        .iter()
        .map(|p| format!(r#"<li><a href="{}" target="_blank" rel="noopener noreferrer">🖼️ {}</a></li>"#, html_escape(&p.src), html_escape(&p.alt)))
        .collect();

    let body = format!(
        r#"<h1>Discover</h1>
<p>登録済みの記事URL群から、動画リンク・テキストリンク・写真を自動収集しています
(PHP側の<code>build_lists()</code>アルゴリズムのRust移植版、1日キャッシュ)。</p>

<h2>動画リンク ({video_count}件)</h2>
<ul class="linklist">{video_items}</ul>

<h2>記事リンク ({text_count}件)</h2>
<ul class="linklist">{text_items}</ul>

<h2>写真 ({photo_count}件)</h2>
<ul class="linklist">{photo_items}</ul>
"#,
        video_count = lists.video_links.len(),
        text_count = lists.text_links.len(),
        photo_count = lists.photos.len(),
    );
    hyper_compat::html_response(StatusCode::OK, page_shell("Discover | audiocafe.tokyo", &body))
}

fn help_body() -> String {
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
    page_shell("困った時は", body)
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt::init();

    // PHP版`--cron-all`(aruaru/index.php 7564〜7649行目)相当。
    // CLI引数で起動された場合はcron処理のみ実行してサーバーは立ち上げず終了する
    // (PHP側は`aruaru_is_cron_request()`でCGI/CLI両対応していたが、Rustバイナリは
    // 常にCLI起動のみを想定すればよいため`std::env::args()`の単純チェックで足りる)。
    if std::env::args().any(|a| a == "--cron-all") {
        cron::run_cron_all().await;
        return Ok(());
    }

    let router = hyper_compat::Router::new()
        .route(
            Method::GET,
            "/",
            Arc::new(|req, _params| {
                Box::pin(async move {
                    let query = hyper_compat::query_params(&req);
                    hyper_compat::html_response(StatusCode::OK, page_shell("AUDIOCAFE | World — Select Your Language", &render_top_body(&query)))
                })
            }),
        )
        .route(Method::GET, "/healthz", Arc::new(|_req, _params| Box::pin(async move { healthz_response() })))
        .route(
            Method::GET,
            "/help",
            Arc::new(|_req, _params| Box::pin(async move { hyper_compat::html_response(StatusCode::OK, help_body()) })),
        )
        .route(Method::GET, "/discover", Arc::new(|_req, _params| Box::pin(async move { discover_page().await })))
        .route(
            Method::GET,
            "/ranking/:slug",
            Arc::new(|_req, params: Params| Box::pin(async move { ranking_page(params).await })),
        )
        .route(
            Method::GET,
            "/page/:slug",
            Arc::new(|_req, params: Params| Box::pin(async move { composite_page(params).await })),
        )
        // PHP版と完全に同じURLパス(`/aruaru/`・`/aruaru-lady/`・
        // `/rakuten-mobile/`)への別名ルート。aruaru.tokyo側のnginxが
        // `Host: audiocafe.tokyo`指定でこれらのパスへ直接プロキシして
        // いるため(`location /aruaru/`等)、`/page/:slug`だけでは
        // 本番切り替え時にaruaru.tokyoが壊れる(404)ことが判明したため
        // 追加した(2026-07-19)。内容は`/page/aruaru`等と同一。
        .route(Method::GET, "/aruaru", Arc::new(|_req, _params| Box::pin(async move { composite_page_by_slug("aruaru").await })))
        .route(Method::GET, "/aruaru-lady", Arc::new(|_req, _params| Box::pin(async move { composite_page_by_slug("aruaru-lady").await })))
        .route(Method::GET, "/rakuten-mobile", Arc::new(|_req, _params| Box::pin(async move { composite_page_by_slug("rakuten-mobile").await })))
        // PHP版の「言語カード＆/top 要約LIST」(`mixlist=1`、`index.php`
        // 1590行目)相当。全カードの主要リンク(YouTube/Blog等)をタイトル付きで
        // 整列表示する専用画面(2026-07-19、ユーザー要望により新設)。
        .route(Method::GET, "/summary", Arc::new(|_req, _params| Box::pin(async move { hyper_compat::html_response(StatusCode::OK, page_shell("AUDIOCAFE | 言語カード要約 — Link Summary", &render_summary_body())) })));

    tracing::info!("audiocafe-tokyo-server listening on 127.0.0.1:4400");
    let (_, handle) = hyper_compat::serve(router, "127.0.0.1:4400".parse().unwrap()).await?;
    handle.await.map_err(|e| std::io::Error::other(e))?;
    Ok(())
}
