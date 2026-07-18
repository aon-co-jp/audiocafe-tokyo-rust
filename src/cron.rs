//! `--cron-all`(PHP側`audiocafe.tokyo/aruaru/index.php` 7564〜7649行目)の
//! Rust移植。8処理のうち**OpenAI API に依存しない4処理**のみ実装する:
//!
//! 1. `rakuten_fetch_price()` — 楽天モバイル基本料金(PHP`rakuten_fetch_price()`、4437行目)
//! 2. `rakuten_intl_crawl()` — 楽天モバイル国際通話(PHP`rakuten_intl_crawl()`、4481行目)
//! 3. `rakuten_platinum_crawl()` — 楽天モバイル プラチナバンド・衛星(PHP`rakuten_platinum_crawl()`、4567行目)
//! 4. `doda_run_crawl()` — doda求人(PHP`doda_run_crawl()`、4831行目)
//!
//! 残り4処理(技術ランキング同期・学習価格・AI学習コメント・英会話ランキング)は
//! `aruaru_tech_refresh_rankings`/`aruaru_learning_ai_cron_refresh`等が
//! OpenAI APIに依存するため今回のスコープ外(`CLAUDE.md`のHANDOFF参照)。
//!
//! PHP版と同じく「失敗時は前回キャッシュ or 安全側デフォルト値を維持」する
//! フェイルセーフ設計を踏襲する(各`*_crawl`関数は取得に失敗しても
//! 常にベースラインの`Value`を返す)。出力先は`--cron-all`実行時の
//! カレントディレクトリ直下(PHPの`__DIR__`相当、ファイル名もPHP側の
//! `*-cache.json`と同名にして、既存`main.rs`の`render_value_generic`が
//! 読むスキーマとそのまま整合させている)。

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::time::Duration;

fn http_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (compatible; AudiocafeBot/1.0; +https://audiocafe.tokyo/)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("reqwest client should build with static config")
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> String {
    match client.get(url).send().await {
        Ok(resp) => resp.text().await.unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// PHPの`rakuten_intl_strip`/プラチナバンド側で重複していたHTML除去処理
/// (`<script>`/`<style>`除去 → タグ除去 → HTMLエンティティ復元 → 空白圧縮)
/// を1関数に集約(PHPは2箇所にほぼ同じコードを書いていた)。
fn strip_html_tags(html: &str) -> String {
    static SCRIPT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap());
    static STYLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap());
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<[^>]+>").unwrap());
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let t = SCRIPT_RE.replace_all(html, "");
    let t = STYLE_RE.replace_all(&t, "");
    let t = TAG_RE.replace_all(&t, " ");
    let decoded = html_escape::decode_html_entities(&t).to_string();
    WS_RE.replace_all(&decoded, " ").trim().to_string()
}

fn today_ymd() -> String {
    chrono::Local::now().format("%Y/%m/%d").to_string()
}

fn now_ymd_hm() -> String {
    chrono::Local::now().format("%Y/%m/%d %H:%M").to_string()
}

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

// ===================== ① 楽天モバイル 基本料金 =====================
// PHP: rakuten_fetch_price() (index.php 4437行目)

fn extract_rakuten_price(html: &str) -> Option<String> {
    static PRICE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([0-9,]+)円[（(]税込[）)]").unwrap());
    PRICE_RE.captures(html).map(|c| format!("{}円（税込）", &c[1]))
}

pub async fn rakuten_fetch_price() -> Value {
    let client = http_client(6);
    let html = fetch_text(&client, "https://network.mobile.rakuten.co.jp/fee/saikyo-plan/").await;
    let price = if html.is_empty() { None } else { extract_rakuten_price(&html) }
        .unwrap_or_else(|| "最大3,278円（税込）".to_string());
    json!({
        "price": price,
        "plan": "Rakuten最強プラン",
        "updated_at": today_ymd(),
        "source_url": "https://network.mobile.rakuten.co.jp/fee/saikyo-plan/",
    })
}

// ===================== ② 楽天モバイル 国際通話 =====================
// PHP: rakuten_intl_crawl() (index.php 4481行目)

fn extract_intl_price_yen(text: &str) -> Option<u32> {
    static PATTERNS: Lazy<[Regex; 2]> = Lazy::new(|| {
        [
            Regex::new(r"月額?\s*([0-9,]+)\s*円[（(]税込[）)]").unwrap(),
            Regex::new(r"([0-9,]+)\s*円[（(]税込[）)]\s*[/／]?月").unwrap(),
        ]
    });
    for re in PATTERNS.iter() {
        if let Some(c) = re.captures(text) {
            if let Ok(n) = c[1].replace(',', "").parse::<u32>() {
                if (300..=5000).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn extract_intl_country_count(text: &str) -> Option<u32> {
    static PATTERNS: Lazy<[Regex; 2]> = Lazy::new(|| {
        [
            Regex::new(r"([0-9]+)\s*[カかヵ]国").unwrap(),
            Regex::new(r"(?i)([0-9]+)\s*countries?").unwrap(),
        ]
    });
    for re in PATTERNS.iter() {
        if let Some(c) = re.captures(text) {
            if let Ok(n) = c[1].parse::<u32>() {
                if (30..=200).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub async fn rakuten_intl_crawl() -> Value {
    let mut price_ja = "月980円（税込）".to_string();
    let mut price_en = "980 yen/month (tax included)".to_string();
    let mut count = "66".to_string();
    let mut cond_ja = vec![
        "渡航前に日本国内で Rakuten Link の認証が必要".to_string(),
        "対象国・地域のみ（約66カ国・地域）".to_string(),
        "一部地域では Wi-Fi 接続が必須".to_string(),
        "0570・0120 など一部番号は無料対象外".to_string(),
        "iPhone は海外着信仕様が Android と一部異なる".to_string(),
        "Rakuten Link を使用した IP 通話方式".to_string(),
    ];
    let mut cond_en = vec![
        "Rakuten Link must be authenticated in Japan before traveling overseas".to_string(),
        "Only supported countries/regions (~66 countries)".to_string(),
        "Wi-Fi may be required in some regions".to_string(),
        "Some numbers (0570/0120 etc.) are excluded".to_string(),
        "iPhone overseas behavior differs slightly from Android".to_string(),
        "Works as IP calling via Rakuten Link app".to_string(),
    ];

    let client = http_client(10);
    let targets = [
        "https://network.mobile.rakuten.co.jp/service/international-call-free/",
        "https://network.mobile.rakuten.co.jp/service/international/",
        "https://network.mobile.rakuten.co.jp/service/rakuten-link/",
    ];
    let mut texts = Vec::new();
    for url in targets {
        let html = fetch_text(&client, url).await;
        if !html.is_empty() {
            texts.push(strip_html_tags(&html));
        }
    }
    let crawl_success = !texts.is_empty();
    if crawl_success {
        let all = texts.join(" ");
        if let Some(n) = extract_intl_price_yen(&all) {
            price_ja = format!("月{}円（税込）", format_thousands(n));
            price_en = format!("{} yen/month (tax included)", format_thousands(n));
        }
        if let Some(c) = extract_intl_country_count(&all) {
            count = c.to_string();
            cond_ja[1] = format!("対象国・地域のみ（約{c}カ国・地域）");
            cond_en[1] = format!("Only supported countries/regions (~{c} countries)");
        }
    }

    json!({
        "intl_plan_price_ja": price_ja,
        "intl_plan_price_en": price_en,
        "intl_countries_count": count,
        "intl_plan_name_ja": "国際通話かけ放題",
        "intl_plan_name_en": "International Unlimited Calling",
        "conditions_ja": cond_ja,
        "conditions_en": cond_en,
        "notes_ja": "月980円の「国際通話かけ放題」は主に「日本→海外」向けですが、Rakuten Link なら海外→日本も無料（対象国・条件あり）。",
        "notes_en": "The 980-yen plan mainly covers Japan→overseas, but Rakuten Link also enables free overseas→Japan calls (conditions apply).",
        "crawled_at": now_ymd_hm(),
        "crawl_success": crawl_success,
    })
}

fn format_thousands(n: u32) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

// ===================== ③ 楽天モバイル プラチナバンド・衛星 =====================
// PHP: rakuten_platinum_crawl() (index.php 4567行目)

fn extract_platinum_coverage(text: &str) -> Option<String> {
    // PHP側は`[^。]{0,60}`(貪欲)だったが、Rustの`regex`も同じ"leftmost-first"
    // semanticsのため貪欲だと数字の途中(例:"99.9"の"9"のみ)を拾ってしまう
    // ケースがあった。ここは非貪欲`{0,60}?`にして、日本語の桁全体を確実に
    // 捕捉できるようにしている(PHP版からの意図的な改善)。
    static PATTERNS: Lazy<[Regex; 2]> = Lazy::new(|| {
        [
            Regex::new(r"プラチナバンド[^。]{0,60}?([0-9]+(?:\.[0-9]+)?)\s*%").unwrap(),
            Regex::new(r"700\s*MHz[^。]{0,60}?([0-9]+(?:\.[0-9]+)?)\s*%").unwrap(),
        ]
    });
    PATTERNS.iter().find_map(|re| re.captures(text).map(|c| c[1].to_string()))
}

fn extract_first_sentence(text: &str, patterns: &[&Regex], min_len: usize, max_len: usize) -> Option<String> {
    for re in patterns {
        if let Some(c) = re.captures(text) {
            let s: String = c[1].trim().chars().take(max_len).collect();
            if s.chars().count() > min_len {
                return Some(s);
            }
        }
    }
    None
}

pub async fn rakuten_platinum_crawl() -> Value {
    let mut platinum_status_ja =
        "700MHz帯プラチナバンドを整備中。地下・屋内・山間部でのつながりやすさを改善。".to_string();
    let mut platinum_coverage_ja = "全国整備進行中（順次拡大中）".to_string();
    let mut platinum_coverage_en = "Nationwide rollout in progress".to_string();
    let mut satellite_status_ja =
        "AST SpaceMobile との提携により、衛星ブロードバンド通話サービスを準備中。".to_string();
    let mut satellite_launch_ja = "商用サービス開始時期は未定（2025〜2026年目標と報道あり）".to_string();

    let client = http_client(10);
    let targets = [
        "https://network.mobile.rakuten.co.jp/",
        "https://network.mobile.rakuten.co.jp/area/",
        "https://corp.rakuten.co.jp/news/press/",
    ];
    let mut texts = Vec::new();
    for url in targets {
        let html = fetch_text(&client, url).await;
        if !html.is_empty() {
            texts.push(strip_html_tags(&html));
        }
    }
    let crawl_success = !texts.is_empty();
    if crawl_success {
        let all = texts.join(" ");
        if let Some(pct) = extract_platinum_coverage(&all) {
            platinum_coverage_ja = format!("人口カバー率 {pct}%（公式より）");
            platinum_coverage_en = format!("Population coverage {pct}% (official)");
        }
        let platinum_re = [
            Regex::new(r"(プラチナバンド[^。]{15,150}。)").unwrap(),
            Regex::new(r"(700\s*MHz[^。]{15,120}。)").unwrap(),
        ];
        let platinum_refs: Vec<&Regex> = platinum_re.iter().collect();
        if let Some(s) = extract_first_sentence(&all, &platinum_refs, 20, 160) {
            platinum_status_ja = format!("{s}（楽天公式より）");
        }
        let sat_re = [
            Regex::new(r"(AST\s*SpaceMobile[^。]{10,180}。)").unwrap(),
            Regex::new(r"(衛星[^。]{5,80}(?:通話|サービス|接続)[^。]{0,60}。)").unwrap(),
        ];
        let sat_refs: Vec<&Regex> = sat_re.iter().collect();
        if let Some(s) = extract_first_sentence(&all, &sat_refs, 15, 200) {
            satellite_status_ja = format!("{s}（公式より）");
        }
        let launch_re = [
            Regex::new(r"(衛星[^。]{0,60}20[2-9][0-9]年[^。]{0,50}。)").unwrap(),
            Regex::new(r"(AST[^。]{0,80}20[2-9][0-9][^。]{0,50}。)").unwrap(),
        ];
        let launch_refs: Vec<&Regex> = launch_re.iter().collect();
        if let Some(s) = extract_first_sentence(&all, &launch_refs, 20, 150) {
            satellite_launch_ja = format!("{s}（公式より）");
        }
    }

    json!({
        "platinum_status_ja": platinum_status_ja,
        "platinum_status_en": "Rakuten Mobile is expanding its 700MHz Platinum Band to improve indoor, underground, and rural coverage.",
        "platinum_coverage_ja": platinum_coverage_ja,
        "platinum_coverage_en": platinum_coverage_en,
        "platinum_detail_ja": "700MHz帯は建物内・地下街まで届きやすい低周波数帯。屋内での通話・データ通信の安定性が向上。",
        "platinum_detail_en": "The 700MHz band penetrates buildings and underground areas more effectively, improving indoor stability.",
        "satellite_status_ja": satellite_status_ja,
        "satellite_status_en": "In partnership with AST SpaceMobile, Rakuten Mobile is developing satellite broadband calling.",
        "satellite_launch_ja": satellite_launch_ja,
        "satellite_launch_en": "Commercial launch TBD (reports suggest 2025-2026 target)",
        "satellite_detail_ja": "低軌道衛星（LEO）により山間部・離島・海上でも通常スマートフォンで通話・データ通信が可能になる見込み。",
        "satellite_detail_en": "LEO satellites will enable calls and data in remote mountains, islands, and offshore areas with standard smartphones.",
        "crawled_at": now_ymd_hm(),
        "crawl_success": crawl_success,
    })
}

// ===================== ④ doda 求人 =====================
// PHP: doda_run_crawl() (index.php 4831行目)

const DODA_MAX_ITEMS: usize = 12;

struct DodaCategoryDef {
    key: &'static str,
    label: &'static str,
    url: &'static str,
}

const DODA_CATEGORIES: &[DodaCategoryDef] = &[
    DodaCategoryDef {
        key: "it",
        label: "IT・通信業界（未経験可／転勤無し）",
        url: "https://doda.jp/DodaFront/View/JobSearchList.action?ss=1&op=17,70,71,27,24&pic=1&ds=0&ind=01L&tp=1&bf=1&mpsc_sid=10&oldestDayWdtno=0&leftPanelType=1",
    },
    DodaCategoryDef {
        key: "ad",
        label: "総合広告代理店／Webマーケティング（広告代理店・コンサル・制作）（未経験可／転勤無し）",
        url: "https://doda.jp/DodaFront/View/JobSearchList.action?ss=1&op=17,70,71,27,24&pic=1&ds=0&ci=131041&ind=1101S,1108S&tp=1&bf=1&mpsc_sid=10&oldestDayWdtno=0&leftPanelType=1",
    },
];

fn is_doda_recommend_url(url: &str) -> bool {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)recommendID=|searchResultFooterAddedArea|_recommend(?:$|[?&/])").unwrap());
    RE.is_match(url)
}

/// jinaの"Image N:"接頭辞除去+空白圧縮(PHP`doda_crawl_category`内の無名関数)。
fn clean_doda_title(raw: &str) -> String {
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    static IMG_PREFIX_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)^!?\[?\s*Image\s*\d+\s*[:：]\s*").unwrap());
    let collapsed = WS_RE.replace_all(raw, " ");
    IMG_PREFIX_RE.replace(collapsed.trim(), "").trim().to_string()
}

struct DodaJobItem {
    title: String,
    url: String,
}

fn doda_extract_items(markdown: &str, max: usize) -> Vec<DodaJobItem> {
    static DETAIL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)https?://doda\.jp/(?:DodaFront/View/JobSearchDetail|jobinfo)/[^\s)\x22]+").unwrap()
    });
    static IMAGE_CARD_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[!\[([^\]]{4,200})\]\([^)]*\)\]\((https?://doda\.jp/[^)\s]+)\)").unwrap());
    static TEXT_LINK_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[([^\]!][^\]]{3,160})\]\((https?://doda\.jp/[^)\s]+)\)").unwrap());

    fn push(
        items: &mut Vec<DodaJobItem>,
        seen: &mut std::collections::HashSet<String>,
        max: usize,
        detail_re: &Regex,
        title: &str,
        url: &str,
    ) -> bool {
        if items.len() >= max {
            return false;
        }
        let title = clean_doda_title(title);
        let url = html_escape::decode_html_entities(url).to_string();
        if title.chars().count() < 4 {
            return true;
        }
        if !detail_re.is_match(&url) {
            return true;
        }
        if is_doda_recommend_url(&url) {
            return true;
        }
        if !seen.insert(url.clone()) {
            return true;
        }
        items.push(DodaJobItem { title, url });
        items.len() < max
    }

    let mut items: Vec<DodaJobItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // (1) 画像付きカード=そのカテゴリの実際の絞り込み結果を優先抽出。
    for c in IMAGE_CARD_RE.captures_iter(markdown) {
        if !push(&mut items, &mut seen, max, &DETAIL_RE, &c[1], &c[2]) {
            break;
        }
    }
    // (2) 件数が不足する場合のみテキストリンクで補完。
    if items.len() < max {
        for c in TEXT_LINK_RE.captures_iter(markdown) {
            if !push(&mut items, &mut seen, max, &DETAIL_RE, &c[1], &c[2]) {
                break;
            }
        }
    }

    items
}

async fn doda_crawl_category(client: &reqwest::Client, url: &str, max: usize) -> Vec<DodaJobItem> {
    let jina_url = format!("https://r.jina.ai/{url}");
    let mut markdown = fetch_text(client, &jina_url).await;
    if markdown.is_empty() {
        markdown = fetch_text(client, url).await; // フォールバック(生HTML)
    }
    if markdown.is_empty() {
        return Vec::new();
    }
    doda_extract_items(&markdown, max)
}

/// 前回キャッシュ(`doda-jobs-cache.json`、カレントディレクトリ直下)を読み、
/// 今回0件だったカテゴリだけ前回の内容へフォールバックする
/// (PHP版と同じ「失敗時は前回キャッシュを維持」というフェイルセーフ)。
fn load_prev_doda_cache() -> Option<Value> {
    let text = std::fs::read_to_string("doda-jobs-cache.json").ok()?;
    rust_json::parse_strict(&text).ok()
}

pub async fn doda_run_crawl() -> Value {
    let client = http_client(25);
    let mut categories = serde_json::Map::new();
    for cat in DODA_CATEGORIES {
        let items = doda_crawl_category(&client, cat.url, DODA_MAX_ITEMS).await;
        let items_json: Vec<Value> = items
            .iter()
            .map(|i| json!({"title": i.title, "url": i.url}))
            .collect();
        categories.insert(
            cat.key.to_string(),
            json!({
                "label": cat.label,
                "search": cat.url,
                "count": items.len(),
                "items": items_json,
            }),
        );
        tokio::time::sleep(Duration::from_millis(600)).await;
    }

    // 0件カテゴリは前回値を温存(失敗で空表示にしない)。
    if let Some(prev) = load_prev_doda_cache() {
        if let Some(prev_categories) = prev.get("categories").and_then(|v| v.as_object()) {
            for (key, cat) in categories.iter_mut() {
                let count_is_zero = cat.get("count").and_then(|v| v.as_u64()) == Some(0);
                if !count_is_zero {
                    continue;
                }
                if let Some(prev_items) = prev_categories.get(key).and_then(|c| c.get("items")).and_then(|v| v.as_array()) {
                    if !prev_items.is_empty() {
                        if let Some(obj) = cat.as_object_mut() {
                            obj.insert("items".to_string(), Value::Array(prev_items.clone()));
                            obj.insert("count".to_string(), json!(prev_items.len()));
                            obj.insert("stale".to_string(), json!(true));
                        }
                    }
                }
            }
        }
    }

    json!({
        "updated": now_rfc3339(),
        "updated_human": now_ymd_hm(),
        "crawled_at": now_rfc3339(),
        "categories": Value::Object(categories),
    })
}

// ===================== --cron-all 統合実行 =====================

fn write_cache_json(filename: &str, data: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(data)?;
    std::fs::write(filename, text)
}

/// PHP版`--cron-all`(index.php 7564〜7649行目)相当の統合実行。
/// OpenAI依存の①③(技術ランキング/AI学習コメント)・英会話ランキング更新は
/// 今回スコープ外のため未実装(CLAUDE.md参照)——4処理のみをこの順で実行する。
pub async fn run_cron_all() {
    let t0 = std::time::Instant::now();
    println!("[{}] ========== audiocafe cron 開始 ==========", now_ymd_hm());

    println!("[{}] [1/4] 楽天モバイル 基本料金 クロール...", now_ymd_hm());
    let rk = rakuten_fetch_price().await;
    if let Err(e) = write_cache_json("rakuten-mobile-cache.json", &rk) {
        eprintln!("[{}] [1/4] 書込エラー: {e}", now_ymd_hm());
    }
    println!(
        "[{}] [1/4] 完了 — 料金: {}",
        now_ymd_hm(),
        rk.get("price").and_then(|v| v.as_str()).unwrap_or("?")
    );

    println!("[{}] [2/4] 楽天モバイル 国際通話 クロール...", now_ymd_hm());
    let intl = rakuten_intl_crawl().await;
    if let Err(e) = write_cache_json("rakuten-intl-call-cache.json", &intl) {
        eprintln!("[{}] [2/4] 書込エラー: {e}", now_ymd_hm());
    }
    println!(
        "[{}] [2/4] 完了 — 国数: {}  成功: {}",
        now_ymd_hm(),
        intl.get("intl_countries_count").and_then(|v| v.as_str()).unwrap_or("?"),
        if intl.get("crawl_success").and_then(|v| v.as_bool()).unwrap_or(false) { "YES" } else { "NO" }
    );

    println!("[{}] [3/4] 楽天モバイル プラチナバンド・衛星 クロール...", now_ymd_hm());
    let plat = rakuten_platinum_crawl().await;
    if let Err(e) = write_cache_json("rakuten-platinum-cache.json", &plat) {
        eprintln!("[{}] [3/4] 書込エラー: {e}", now_ymd_hm());
    }
    println!(
        "[{}] [3/4] 完了 — カバレッジ: {}  成功: {}",
        now_ymd_hm(),
        plat.get("platinum_coverage_ja").and_then(|v| v.as_str()).unwrap_or("?"),
        if plat.get("crawl_success").and_then(|v| v.as_bool()).unwrap_or(false) { "YES" } else { "NO" }
    );

    println!("[{}] [4/4] doda 求人 クロール...", now_ymd_hm());
    let doda = doda_run_crawl().await;
    if let Err(e) = write_cache_json("doda-jobs-cache.json", &doda) {
        eprintln!("[{}] [4/4] 書込エラー: {e}", now_ymd_hm());
    }
    let it_count = doda.get("categories").and_then(|c| c.get("it")).and_then(|c| c.get("count")).and_then(|v| v.as_u64()).unwrap_or(0);
    let ad_count = doda.get("categories").and_then(|c| c.get("ad")).and_then(|c| c.get("count")).and_then(|v| v.as_u64()).unwrap_or(0);
    println!("[{}] [4/4] 完了 — IT={it_count} AD={ad_count}", now_ymd_hm());

    println!(
        "[{}] ========== audiocafe cron 完了（{:.2}秒） ==========",
        now_ymd_hm(),
        t0.elapsed().as_secs_f64()
    );
    println!(
        "[{}] ※ 技術ランキング/AI学習コメント/英会話ランキング(OpenAI API依存)は今回未実装、CLAUDE.md参照",
        now_ymd_hm()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_rakuten_price_matches_yen_with_tax_label() {
        assert_eq!(
            extract_rakuten_price("最強プランは3,278円（税込）です"),
            Some("3,278円（税込）".to_string())
        );
        assert_eq!(extract_rakuten_price("価格情報なし"), None);
    }

    #[test]
    fn extract_intl_price_yen_rejects_out_of_range_numbers() {
        assert_eq!(extract_intl_price_yen("月額980円（税込）でご利用いただけます"), Some(980));
        assert_eq!(extract_intl_price_yen("10000円（税込）"), None); // 範囲外(5000超)
        assert_eq!(extract_intl_price_yen("特にありません"), None);
    }

    #[test]
    fn extract_intl_country_count_accepts_kanji_and_english_units() {
        assert_eq!(extract_intl_country_count("対象は約66カ国です"), Some(66));
        assert_eq!(extract_intl_country_count("covers 70 countries worldwide"), Some(70));
        assert_eq!(extract_intl_country_count("5カ国のみ"), None); // 範囲外(30未満)
    }

    #[test]
    fn extract_platinum_coverage_matches_percentage_near_keyword() {
        assert_eq!(
            extract_platinum_coverage("プラチナバンドの人口カバー率は99.9%に達しました"),
            Some("99.9".to_string())
        );
        assert_eq!(extract_platinum_coverage("無関係な文章です"), None);
    }

    #[test]
    fn is_doda_recommend_url_flags_common_recommend_patterns() {
        assert!(is_doda_recommend_url("https://doda.jp/jobinfo/x?recommendID=123"));
        assert!(is_doda_recommend_url("https://doda.jp/jobinfo/x?searchResultFooterAddedArea=1"));
        assert!(!is_doda_recommend_url("https://doda.jp/DodaFront/View/JobSearchDetail/abc"));
    }

    #[test]
    fn clean_doda_title_strips_jina_image_prefix_and_collapses_whitespace() {
        assert_eq!(clean_doda_title("Image 3:  ITエンジニア   求人"), "ITエンジニア 求人");
        // 全角スペース(U+3000)もUnicodeの空白として\s+に含まれ、半角1つに正規化される。
        assert_eq!(clean_doda_title("広告代理店　プランナー"), "広告代理店 プランナー");
    }

    #[test]
    fn doda_extract_items_prefers_image_cards_and_excludes_recommend_links() {
        let md = r#"
[![Image 1: ITエンジニア募集](https://img.example/1.png)](https://doda.jp/DodaFront/View/JobSearchDetail/12345)
[![Image 2: おすすめ求人](https://img.example/2.png)](https://doda.jp/DodaFront/View/JobSearchDetail/999?recommendID=1)
[広告プランナー募集中です](https://doda.jp/jobinfo/67890)
        "#;
        let items = doda_extract_items(md, 12);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "ITエンジニア募集");
        assert_eq!(items[0].url, "https://doda.jp/DodaFront/View/JobSearchDetail/12345");
        assert_eq!(items[1].title, "広告プランナー募集中です");
    }

    #[test]
    fn format_thousands_inserts_commas() {
        assert_eq!(format_thousands(980), "980");
        assert_eq!(format_thousands(1234), "1,234");
        assert_eq!(format_thousands(1234567), "1,234,567");
    }
}
