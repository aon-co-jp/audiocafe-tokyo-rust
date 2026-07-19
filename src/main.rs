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

/// PHP側の`rakuten-mobile/index.php`(917行、`rm_render_fragment()`)が
/// 実際に表示している専用ページの内容をそのまま移植する。汎用JSONダンプ
/// (`render_value_generic`)ではPHP版と全く別のページになってしまうため
/// (2026-07-19監査で判明)、この関数はPHP版のセクション構成・見出し・
/// 静的マーケティング文言を1対1で再現しつつ、データ部分(料金・国際通話・
/// プラチナバンド/衛星)は既存の`fetch_cache`アーキテクチャ経由で取得した
/// 3キャッシュJSONから埋め込む。
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
        r#"<h1>📶 楽天モバイル 最新情報</h1>
<p>自社の楽天回線エリアとau回線（パートナー回線）エリアを合わせてデータ使い放題（パケット放題）となります。</p>
<p>月間無制限に使っても <strong>{price}</strong>（税込）</p>
<p><a href="{official_url}" target="_blank" rel="noopener noreferrer">公式サイトで確認 →</a></p>
<p class="disclaimer">📅 {updated_at}</p>

<h2>📡 楽天回線エリア</h2>
<p>人口カバー率<strong>99.9%</strong>を達成。自社基地局エリア内ではデータ高速<strong>無制限</strong>で利用できます。</p>

<h2>🔄 パートナー回線（au）エリア</h2>
<p>楽天電波が届きにくい屋内や一部エリアでは<strong>auローミング</strong>を利用。月間<strong>5GBまで</strong>高速、超過後は最大1Mbps。</p>

<h2>⚠️ 注意点</h2>
<p>地下・高層ビル・奥まった屋内では繋がりにくい場合あり。プラチナバンド（700MHz帯）を拡大中。</p>

<h2>🗺️ エリア確認ツール</h2>
<ul>
<li><a href="{area_url}" target="_blank" rel="noopener noreferrer">📍 楽天モバイル 通信・エリアマップ</a></li>
<li><a href="{area_faq_url}" target="_blank" rel="noopener noreferrer">❓ データ高速無制限エリアとは</a></li>
</ul>

<h2>🔍 関連検索</h2>
<ul>
<li><a href="{price_search_url}" target="_blank" rel="noopener noreferrer">🔍 最新料金を Google で検索</a></li>
<li><a href="{campaign_url}" target="_blank" rel="noopener noreferrer">🔍 乗り換えキャンペーン</a></li>
<li><a href="{we2plus_url}" target="_blank" rel="noopener noreferrer">📱 1円スマホ（we2 plus）</a></li>
</ul>

<h2>📞 楽天モバイル 国際通話プラン詳細</h2>
<p class="disclaimer">📅 {intl_crawled}{intl_ok_note}</p>
<p>
🇯🇵 日本 → 海外 プラン料金：{intl_price} / {intl_name}<br>
🌍 かけ放題対象国：{intl_count} カ国<br>
✈️ 海外 → 日本：Rakuten Link 利用時 無料（対象国・条件あり）
</p>
<p><strong>🌏 海外からも日本へ電話放題？</strong><br>
✅ はい、かなり本当です。主に Rakuten Link アプリ利用時（条件あり）。</p>
<p>
🇯🇵 日本→日本：Rakuten Link で無料<br>
🇯🇵 日本→海外：「{intl_name}（{intl_price}）」で{intl_count}カ国かけ放題<br>
✈️ 海外→日本：Rakuten Link で無料（対象国から）
</p>
<p><a href="{intl_free_url}" target="_blank" rel="noopener noreferrer">📎 国際通話かけ放題 公式ページ</a></p>

<h2>🚀 衛星ブロードバンド通話（AST SpaceMobile 提携）</h2>
<p class="disclaimer">📅 {plat_crawled}{plat_ok_note}</p>
<p>{sat_status}<br>{sat_detail}<br><span class="disclaimer">🛰️ {sat_launch}</span></p>

<h2>📡 プラチナ回線（700MHz帯 プラチナバンド）</h2>
<p class="disclaimer">📅 {plat_crawled}{plat_ok_note}</p>
<p>{plat_status}<br>{plat_detail}<br><span class="disclaimer">📶 カバレッジ：{plat_coverage}</span></p>

<h2>📶 楽天モバイル（1円スマホ・パケット放題・電話放題）</h2>
<p>スマホなら楽天モバイルへの乗り換えを検討できます。eSIM 対応端末やキャンペーンの一例として、富士通製「we2 plus」など高性能 CPU 端末を<strong>1円</strong>で入手できる案内が出る場合があります（時期・在庫・契約条件は要確認）。日本全国で<strong>楽天リンク</strong>アプリが使えます。</p>
<ul>
<li><a href="{we2plus_url}" target="_blank" rel="noopener noreferrer">1円スマホの例：富士通製 we2 plus など（要確認）</a></li>
<li><a href="{packet_url}" target="_blank" rel="noopener noreferrer">パケット放題・データ使い放題プラン</a></li>
<li><a href="{phone_url}" target="_blank" rel="noopener noreferrer">電話放題・楽天リンク経由の通話</a></li>
<li><a href="{link_android_url}" target="_blank" rel="noopener noreferrer">楽天リンク Android版</a></li>
<li><a href="{link_iphone_url}" target="_blank" rel="noopener noreferrer">楽天リンク iPhone版</a></li>
<li><a href="{campaign_url}" target="_blank" rel="noopener noreferrer">楽天モバイル 乗り換え・キャンペーン全般</a></li>
</ul>
<p class="disclaimer">楽天モバイルのアンテナ・基地局が届くエリアでは、オンライン配信や TV チャットでも<strong>パケットを気にしにくいプラン</strong>を検討できます。病院などの FREE Wi-Fi が使える場合、在宅・入院中の環境づくりにも役立つことがあります（プラン内容・エリアは必ず公式で確認してください）。</p>

<h2>⏱ 自動更新について</h2>
<p class="disclaimer">このページの元データ(楽天モバイル料金・国際通話・プラチナバンド)はPHP版サイトが毎朝05:00AMに自動クロール・キャッシュ更新しています。キャッシュ先: rakuten-mobile-cache.json 他2ファイル。</p>

<p style="margin-top:2rem;">
<a href="/">← audiocafe.tokyo トップ</a> ・
<a href="/aruaru">📊 aruaru（IT技術情報）</a> ・
<a href="/aruaru-lady">💃 aruaru-lady（女性向け情報）</a> ・
<a href="{ARUARU_TOKYO_URL}" target="_blank" rel="noopener noreferrer">🎲 aruaru.tokyo</a>
</p>
<p class="disclaimer">楽天モバイル情報は毎朝05:00AMに自動クロール更新。内容は必ず<a href="{official_url}" target="_blank" rel="noopener noreferrer">公式サイト</a>でご確認ください。</p>
"#,
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
