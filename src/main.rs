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
<nav><a href="/">TOP</a> <a href="/discover">Discover</a> <a href="/help">困った時は</a> <a href="{ARUARU_TOKYO_URL}">aruaru.tokyo</a></nav>
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

    let doda_categories: String = [("it", "💼 IT・通信業界(未経験可／転勤無し)"), ("ad", "📢 広告・マーケティング業界(未経験可／転勤無し)")]
        .iter()
        .map(|(key, fallback_label)| {
            let cat = doda.get("categories").and_then(|c| c.get(key));
            let label = cat.and_then(|c| c.get("label")).and_then(|v| v.as_str()).unwrap_or(fallback_label).to_string();
            let search = cat.and_then(|c| c.get("search")).and_then(|v| v.as_str()).unwrap_or("https://doda.jp/").to_string();
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
<h2 style="color:#00ffff;">🚀 人気TOP80 技術ランキング（AI自動分析）</h2>
<p style="opacity:.7;font-size:15px;">🕐 最終更新: {tech_updated}</p>
<h3 style="color:#00ffaa;">💻 人気プログラミング言語 TOP80</h3>
{lang_table}
<h3 style="color:#ffaa00;">⚡ 人気フレームワーク TOP80</h3>
{fw_table}
<h3 style="color:#ff66cc;">🗄 DATABASE ランキング TOP80</h3>
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

fn top_body() -> String {
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
    page_shell("audiocafe.tokyo (Rust移行版)", &body)
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
            Arc::new(|_req, _params| Box::pin(async move { hyper_compat::html_response(StatusCode::OK, top_body()) })),
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
        .route(Method::GET, "/rakuten-mobile", Arc::new(|_req, _params| Box::pin(async move { composite_page_by_slug("rakuten-mobile").await })));

    tracing::info!("audiocafe-tokyo-server listening on 127.0.0.1:4400");
    let (_, handle) = hyper_compat::serve(router, "127.0.0.1:4400".parse().unwrap()).await?;
    handle.await.map_err(|e| std::io::Error::other(e))?;
    Ok(())
}
