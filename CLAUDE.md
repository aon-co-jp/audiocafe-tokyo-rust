# 開発方針＆開発環境ルール(audiocafe-tokyo-server)

作業ドライブは`F:\open-runo`。この節は[`open-raid-z`](https://github.com/aon-co-jp/open-raid-z)の`CLAUDE.md`を正本とし、各プロジェクトへコピーして同期する方針に準じる。

## このリポジトリの役割(2026-07-17新設、PHP→Rust移行 第一段)

`audiocafe.tokyo`の既存PHPモノリス([`audiocafe-tokyo`](https://github.com/aon-co-jp/audiocafe-tokyo)リポジトリ、`index.php`単体445KB・189関数)を
Rust + Poemへ段階的に移行するための新サイト。既存PHPリポジトリは上書きせず、
このリポジトリは独立した移行先として新設した。

[`aruaru-tokyo-server`](https://github.com/aon-co-jp/aruaru-tokyo-server)・
[`aon-tokyo`](https://github.com/aon-co-jp/aon-tokyo)・
[`karu-tokyo`](https://github.com/aon-co-jp/karu-tokyo)と同じ技術スタック
(Rust+Poem、DB非依存、1バイナリ完結、テンプレートエンジン不使用)。

## 移行方針・現状の対応範囲(正直な開示)

既存PHP側の調査(2026-07-17)の結果、実データの大半は`*-cache.json`
(ファイルベースキャッシュ、`https://audiocafe.tokyo/`直下に静的公開済み、
DB接続はほぼ無い)によるジャンル別ランキング表示であることが判明した。
このRust側はそのキャッシュJSONを**HTTP経由で取得**し(サーバー間の直接
ファイル共有を前提にしない疎結合設計)、
[`rust_json::parse_strict`](https://github.com/aon-co-jp/Rust-JSON)でパースして
汎用的にレンダリングする。

キャッシュのスキーマは8種類あり完全には統一されていなかったが、
2026-07-17に**完全再帰の汎用レンダラー**(`render_value_generic`、
`src/main.rs`)へ書き換え、**全8種類に対応済み**:

- スカラー値(文字列/数値/真偽値) → `<p>`
- URL文字列 → クリック可能なリンク
- 文字列配列 → `<ul>`
- 同一キー構成のオブジェクト配列(地域別ランキングの`rows`、
  `ai-tech-ranking`の`languages`/`frameworks`/`databases`等) → 表
- キー構成が揃わないオブジェクト配列 → 箇条書きへフォールバック
- ネストしたオブジェクト(地域別の`tokyo_23`等、`sources`等) → 再帰

形状ごとに専用コードを書く必要がなくなったため、8種全て
(`aruaru-caba`・`aruaru-eikaiwa`・`aruaru-jukujo-caba`・
`ai-tech-ranking`・`aruaru-learning-prices`・`rakuten-mobile`・
`rakuten-intl-call`・`rakuten-platinum`)を1つの実装でカバーする。

## 複合ページ(2026-07-17、`/page/:slug`)

既存PHP側の`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`は、実は単一の
ランキングではなく**複数のキャッシュファイルを1ページに束ねた統合ページ**
であることが判明した(VPS上の実ファイル構成を調査して確認)。例えば
`/aruaru/`は10種類のキャッシュ(自ジャンルのランキング4種+楽天モバイル
関連4種+doda求人)を1ページにまとめている。

`COMPOSITE_PAGES`(`src/main.rs`)にセクション見出しとキャッシュの相対
パス(サブディレクトリ込み、例: `aruaru/rakuten-mobile-cache.json`)を
列挙し、`render_composite_body`が各セクションを`render_value_generic`で
順に描画する——新しい形状の専用コードは今回も追加していない
(既存の汎用レンダラーがそのまま新規キャッシュ`rakuten-smartphone`・
`doda-jobs`・`aruaru-tvchat-normal/group`にも通用することを確認)。

`/page/rakuten-mobile`・`/page/aruaru`・`/page/aruaru-lady`として提供。

**まだ移植していないもの(ごまかさず明記)**:
- 元のPHPページのHTML構造・デザイン・レイアウト(装飾、画像配置等)は
  再現していない(機能等価の最小限HTMLのみ)。
- 多言語版(`index-en.php`・`index-fr.php`等、`/aruaru/`だけで12言語)は
  未対応。日本語版相当のみ。
- cron相乗りによるキャッシュ自動更新ロジック自体(PHP側の
  `--cron-all`等)は移植していない——Rust側は既存のキャッシュファイルを
  読むだけで、更新は引き続きPHP側のcronに依存している。

## 検証(2026-07-17)

VPS上(実インターネット接続あり)で`cargo build`→実バイナリ起動→
`curl`で`/`・8種類全ての`/ranking/:slug`ルートすべて200、かつ実際に
本番の`https://audiocafe.tokyo/*-cache.json`を取得して
(地域別ランキング表・`ai-tech-ranking`の78言語表・`rakuten-intl-call`の
日英併記フィールド・`aruaru-learning-prices`の月額料金等)それぞれ
正しくレンダリングされることを確認済み(型チェックのみでの「完了」
報告ではなく、実データでの動作確認)。

## デプロイ(2026-07-17、プレビュー公開まで完了)

VPS上`/root/audiocafe-tokyo-rust`(GitHubからclone、git管理下)で
`cargo build --release`、systemdサービス化(`audiocafe-tokyo-rust.service`、
`127.0.0.1:4400`)。**既存PHPサイト本番(`https://audiocafe.tokyo/`)は
一切変更せず**、`audiocafe.tokyo.conf`の443番serverブロックに
`location /rust-preview/`だけを追加し、そこだけ`127.0.0.1:4400`へ
プロキシする形で並行公開した。

`https://audiocafe.tokyo/` (PHP本番、既存のまま) と
`https://audiocafe.tokyo/rust-preview/` (Rust版プレビュー) の両方が
実際に200を返すことを確認済み——本番を壊さずに新実装を試せる状態。

既存`audiocafe.tokyo`のnginx vhostは、`aruaru.tokyo`から
`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`が内部プロキシで参照
している実体でもあるため、それらのパスの扱いを崩さないよう
本カットオーバー(`location /`自体をRust版に切り替える)は別途
計画・確認してから行う。

## 関連プロジェクト

- [audiocafe-tokyo](https://github.com/aon-co-jp/audiocafe-tokyo) — 既存PHP実装(移行元)
- [Rust-JSON](https://github.com/aon-co-jp/Rust-JSON) — JSON解析に使用
- [aruaru-tokyo-server](https://github.com/aon-co-jp/aruaru-tokyo-server) — 技術スタックの出典元

## 運用ルール追記(2026-07-18、正本はopen-raid-zのCLAUDE.md参照) — 確認不要の自動継続・リミット解除後の自動再開

- **コンテキストウインドウ・5時間利用制限・その他のセッション中断が
  発生し、その後リミットが解除されて新しいセッションが開始された場合、
  「続けてよろしいですか」等の確認を挟まず、毎回自動的に前回セッションの
  続きの作業を再開すること**(ユーザー指示、2026-07-18)。具体的には:
  1. セッション開始時、各リポジトリの`git status`/`git log`と、この
     `CLAUDE.md`(および他プロジェクトのCLAUDE.md)のHANDOFF節・
     「次にすべきこと」記載を確認し、未完了・未pushの作業が無いかを
     まず裏取りする(タスク管理メタデータを鵜呑みにしない既存方針と
     同じ姿勢で、実際のgit状態を確認する)。
  2. 未完了作業が見つかった場合、ユーザーへの確認を求めず、そのまま
     自動的に検証(build/test)→修正→コミット→pushまで完了させる。
  3. 完了している場合は、各CLAUDE.mdの「次にすべきこと」「未着手・
     未完成」に記載された次の項目へ確認なしに着手する(既存の
     「未着手だからといって確認を求めて手を止めない」方針の延長)。
  4. 「続けてよろしければそのまま自動開発を継続します」のような、
     続行そのものを尋ねる確認は今後一切行わない(ユーザー指示、
     2026-07-18)。作業内容の要約・進捗報告はしてよいが、それは
     承認を求めるものではなく完了報告として書く。
  5. こまめにコミット・pushしておくことで、次回セッションが「どこから
     再開すべきか」を迷わず`git log`/CLAUDE.mdから機械的に判断できる
     ようにしておく(区切りがついた時点で都度コミット・pushする既存
     方針との組み合わせ)。


## HANDOFF

- **2026-07-18(続きの続きの続き) Poem → RPoem`open-runo-router::hyper_compat`への移行完了
  (エコシステム方針、Poem/Tauriパッケージへの直接依存を断つ)**:
  前セッションが`Cargo.toml`の`poem = "3.1"`を
  `open-runo-router = { path = "../RPoem/crates/open-runo-router" }`に
  書き換えた状態で中断しており、`src/main.rs`側は旧`poem`のAPI
  (`poem::web::{Html, Path}`・`#[handler]`・`Route::new().at(...)`・
  `Server::new(TcpListener::bind(...))`)のままでコンパイル不能な状態
  だった。これを`F:\open-runo\RPoem\crates\open-runo-router\src\
  hyper_compat.rs`(手書きtokio/hyperルーター、Poem本体には非依存)を
  使う形に書き換えた——「Poem/Tauriをパッケージとして直接依存させず、
  RPoem側の自前実装(API形状は互換)を使う」というエコシステム全体の
  方針([`RPoem/CLAUDE.md`](https://github.com/aon-co-jp/RPoem)参照)を、
  このリポジトリにも適用する対応。
  - **変更点**: `Cargo.toml`に`hyper`(1.10, full features)・`bytes`
    (1.9)を追加(`hyper_compat`のResponse/Request型・`Method`/
    `StatusCode`・`fixed_body`を直接使うため必要)。`src/main.rs`は
    `#[handler]`マクロ・`Html<String>`・`Path<String>`extractorを廃止し、
    各ハンドラを素の関数(`top_body() -> String`・
    `ranking_page(params: Params) -> hyper_compat::Response`等)に書き換え、
    `hyper_compat::Router::new().route(Method::GET, "/ranking/:slug",
    Arc::new(|_req, params| Box::pin(async move { ... })))`という
    クロージャ登録形式でルート定義。`/healthz`は`hyper_compat`に
    プレーンテキスト専用ヘルパーが無かったため、`hyper::Response::builder()`
    +`hyper_compat::fixed_body`(pub化済みヘルパー)で
    `text/plain; charset=utf-8`の200レスポンスを直接組み立てた。
    `main()`の起動部分は`hyper_compat::serve(router, addr).await`+
    返り値の`JoinHandle`を`.await`する形に変更(元の`Server::run(app).await`
    と同じブロッキング挙動を維持)。`--cron-all`のCLI早期リターンは無変更。
    `src/cron.rs`・`src/scraper.rs`・`src/seed_urls.rs`はいずれも
    未変更(Poem/hyper_compatに依存していないため対象外)。
  - **検証**: `cargo build`成功(エラー・警告なし、`open-runo-router`側の
    既存3警告(`missing_debug_implementations`、無関係)のみ)。
    `cargo test`で14件全green
    (`cron`/`scraper`のテストのみ、HTTP層に変更前後の差なし——タスク
    冒頭の想定は15件だったが実際は14件、いずれにせよ全件成功)。
    実バイナリを起動し`curl`で`/`・`/healthz`(`ok`、200)・`/help`・
    `/discover`(360件シードURLの実クロール)・
    `/ranking/aruaru-eikaiwa`(実際に`https://audiocafe.tokyo/
    aruaru-eikaiwa-ranking-cache.json`を取得し50件の表を描画)・
    `/page/rakuten-mobile`(複合ページ、3セクション実データ)を確認、
    全て200・`<nav>`/`<h1>`/`<h2>`を含む正しいHTML構造であることを
    確認済み(型チェックのみでの完了報告ではない)。検証後サーバー
    プロセスは停止済み。
  - 次にすべきこと: 特に緊急の課題はない。今後、RPoemの
    `hyper_compat`にHANDOFF記載のような新機能(gRPC・MCP等)が追加
    された場合、このリポジトリ側で使う必要が生じれば追従を検討する
    (現状は素朴なGETルーティングのみで十分)。

- **2026-07-18(続きの続き) 英会話ランキング更新処理を追加実装、`--cron-all`を非AI処理5件に拡張**:
  従来「OpenAI API依存で今回スコープ外」と誤って記録されていた
  `aruaru_eikaiwa_ranking_refresh()`(PHP`index.php`1902行目)を
  再調査した結果、**実際には完全に静的なハードコードデータ
  (`aruaru_eikaiwa_master_pool()`、1820行目、英会話アプリ・サービス
  TOP50)を`rank`でソートしてJSON書き出すだけの非AI処理**であることが
  判明した(各行の`'ai'=>true/false`はそのサービス自体がAI機能を
  持つか否かの表示用フラグであり、この処理自体がOpenAI APIを
  呼ぶわけではない)。よって`src/cron.rs`に`EIKAIWA_POOL`(50件の
  静的タプル配列、PHP版データをそのまま移植)+`eikaiwa_ranking_refresh()`
  を追加し、`run_cron_all()`の`[5/5]`として組み込んだ(既存4処理は
  `[n/4]`→`[n/5]`表記へ更新)。出力先は`aruaru-eikaiwa-ranking-cache.json`
  (PHP版と同名、`main.rs`の`render_value_generic`が既に読むスキーマと
  完全一致)。
  - **検証**: `cargo build`成功、`cargo test`で新規1件+既存13件の
    計14件全green(新規テストは50件全件・rank昇順ソート・`ttl_days=7`・
    先頭行の内容を検証)。さらに実バイナリで`--cron-all`を実行し、
    既存4処理(楽天3種+doda)も含め全5処理が正常完了することを確認
    (楽天基本料金「最大3,278円」、国際通話「66カ国」成功、プラチナ
    バンド「全国整備進行中」成功、doda IT=12/AD=12、英会話50件)。
    生成された`aruaru-eikaiwa-ranking-cache.json`の中身も確認し、
    `rows[0]`が`Duolingo`(rank=1)であることを含めPHP版と一致することを
    確認済み(型チェックのみでの完了報告ではない)。確認後、一時実行
    ディレクトリは削除済み。
  - **今回のスコープ外(継続)**: 技術ランキング同期・AI学習コメントの
    2処理(いずれも実際にOpenAI APIで自然文・実在URLを新規生成する
    処理)は引き続き未実装。cronのスケジュール実行自体(systemd timer/
    cron設定)もVPS側の運用作業として別途必要。
  - 次にすべきこと: (1) VPSへの本番デプロイ時に`--cron-all`の出力先
    ディレクトリを決定し、systemd timer等で毎日実行するよう設定する、
    (2) 必要であれば技術ランキング/AI学習コメント(OpenAI依存)にも
    着手する。

- **2026-07-18(続き) cron自動更新ロジック、OpenAI非依存4処理を実装完了**:
  下記調査ログの「次にすべきこと」を実施し、新規`src/cron.rs`に
  楽天モバイル3種(基本料金/国際通話/プラチナバンド)+doda求人の
  4処理を実装、`src/main.rs`に`--cron-all`のCLI引数判定
  (`std::env::args()`、PHPの`aruaru_is_cron_request()`のCGI/CLI両対応の
  複雑さは不要と判断しシンプルな文字列一致のみ)を追加した。
  - **実装方式**: 各処理はPHPの`rakuten_fetch_price`/`rakuten_intl_crawl`/
    `rakuten_platinum_crawl`/`doda_run_crawl`(いずれも`audiocafe.tokyo/aruaru/index.php`)
    のロジックをほぼそのまま`reqwest`+`regex`へ移植。正規表現抽出・
    「失敗時は前回キャッシュ or 安全側デフォルト値を維持」という
    フェイルセーフ設計もPHP版を踏襲。doda求人は`r.jina.ai`経由のmarkdown
    抽出→失敗時は生HTMLへフォールバックという既存方針(PHP側と同じ)のまま。
  - **PHP版からの意図的な差分1点**: プラチナバンドのカバー率抽出正規表現
    (`extract_platinum_coverage`)は、PHP版の貪欲`[^。]{0,60}`のままだと
    実際に"99.9%"のような小数点付き数値の頭が欠けて"9"だけを拾ってしまう
    ケースをテストで発見したため、Rust版は非貪欲`{0,60}?`に変更して
    修正済み(`src/cron.rs`内にコメントで明記)。
  - **出力先**: `--cron-all`実行時のカレントディレクトリ直下に
    PHP版と同名の`rakuten-mobile-cache.json`・`rakuten-intl-call-cache.json`・
    `rakuten-platinum-cache.json`・`doda-jobs-cache.json`を書き出す
    (本番配置先ディレクトリの決定・systemd timer/cron設定は運用面の
    別タスクとして未着手)。`.gitignore`に`/*-cache.json`を追加し、
    ローカル実行で生成されるファイルがリポジトリに混入しないようにした。
  - **検証**: `cargo build`成功、`cargo test`で新規8件+既存5件の
    計13件全green。さらに実インターネット接続がある開発環境で実際に
    `audiocafe-tokyo-server.exe --cron-all`を実行し、4処理とも実際に
    外部サイト(楽天モバイル公式・doda)へアクセスして正しいスキーマの
    JSONを生成することを確認済み(型チェックのみでの完了報告ではない)——
    楽天基本料金「最大3,278円（税込）」、国際通話「66カ国」成功、
    プラチナバンド「全国整備進行中」成功、doda求人IT=12件/AD=12件を
    実データで取得。確認後、生成された一時キャッシュファイルは削除済み。
  - **今回のスコープ外(継続)**: 技術ランキング同期・AI学習コメント・
    英会話ランキングの3処理(いずれもOpenAI API依存、または今回未着手)は
    未実装のまま。cronのスケジュール実行自体(systemd timer/cron設定)も
    VPS側の運用作業として別途必要。
  - 次にすべきこと: (1) VPSへの本番デプロイ時に`--cron-all`の出力先
    ディレクトリ(nginx静的公開ディレクトリ)を決定し、systemd timer等で
    毎日実行するよう設定する、(2) 必要であれば技術ランキング/AI学習コメント
    (OpenAI依存)にも着手する。

- **2026-07-18 cron自動更新ロジック(`--cron-all`相当)の調査完了、移植は設計方針の
  記録に留めた(スコープ大につき見送り)**: 前回セッションが通信エラーで中断した
  `cron.php`調査を引き継ぎ、`F:\open-runo\audiocafe.tokyo`のローカルPHPコピーを
  精査した。未コミット変更は無く(`git status`クリーン)、`cargo build --release`・
  `cargo test`(5件green)・実バイナリ起動での全ルート(`/`・`/healthz`・
  `/ranking/:slug`全8種・`/page/:slug`全3種・`/discover`・`/help`)200確認、
  実データ(78言語の技術ランキング表・980円国際通話プラン・150KB超の複合ページ
  11テーブル)のレンダリングも確認済み——既存実装に劣化は無い。
  - **cron.phpの実体**: `audiocafe.tokyo/aruaru/cron.php`と`aruaru-lady/cron.php`は
    どちらも`$argv=['cron.php','--cron-all']`を仕込んで`require __DIR__.'/index.php'`
    するだけの薄いラッパー。実処理は`aruaru/index.php`(8152行、姉妹ファイルであり
    Rust側が既に移植した旧ドライブ直下の`index.php`8146行とは別物)の末尾
    (7514〜7649行目)にある`--cron-all`統合ブロック。
  - **`aruaru_is_cron_request()`(28行目付近)**: LOLIPOPのcronがCLIではなくCGI
    (`cgi-fcgi`)で起動される場合があるため、`$argv`/`$_SERVER['argv']`両経路+
    「HTTP経由でない(`$no_http`)」判定を組み合わせてcron起動を検知する独自関数。
    個別フラグ(`--cron-intl`/`--cron-platinum`/`--cron-doda`)と統合フラグ
    (`--cron-all`、または引数なしCLI直接実行)の両方に対応。
  - **`--cron-all`が呼ぶ8処理(7564〜7649行目)**といずれも外部サイトへの
    実クロール・API呼び出しを伴う:
    1. `aruaru_tech_refresh_rankings()`(787行目) — Stack Overflow/DB-Engines等
       外部ソースから言語・FW・DB各80件の人気度を同期し、`OPENAI_API_KEY`
       設定時はOpenAI APIで紹介文(`ai_comment`)を生成(未設定ならベースライン
       固定文言にフォールバック)。`ai-tech-ranking-cache.json`へ保存。
    2. `aruaru_learning_prices_refresh()`(1723行目) — 学習サービス価格情報更新。
    3. `aruaru_learning_ai_cron_refresh()`(1511行目) — AI学習コメントを1回の
       cronにつき最大12件ずつ小分け更新(レート制限対策と思われる)。
    4. `aruaru_eikaiwa_ranking_refresh()`(1902行目) — 英会話アプリ・サービス
       ランキング(週1回7日TTL)。
    5〜7. `rakuten_fetch_price()`/`rakuten_intl_crawl()`/`rakuten_platinum_crawl()`
       (4437・4481・4567行目) — 楽天モバイル公式サイトを`file_get_contents`+
       正規表現でスクレイプ(基本料金・国際通話・プラチナバンド)。失敗時は
       前回キャッシュまたは安全側デフォルト値へフォールバック。
    8. `doda_run_crawl()`(4831行目) — doda求人サイトをIT/広告代理店カテゴリで
       クロールし件数集計。
    - 各処理は個別に「失敗時は前回キャッシュ or 安全側デフォルトを維持」する
      設計(例: `campaign_active=true`固定などフェイルセーフ)。
    - 最後に対象キャッシュファイルの`mtime`を強制`touch`し、内容が同一でも
      画面上の「最終更新」バッジが毎朝反映されるようにしている。
  - **Rust側への移植方針(設計のみ、未実装)**:
    - 現行Rust実装は「PHP側が生成した`*-cache.json`をHTTP経由で読むだけ」の
      疎結合設計(`main.rs`)であり、この設計自体は cron 自動更新ロジックの
      移植と両立する——cron側を移植してもキャッシュ取得部分は変更不要。
    - 移植する場合の想定構成: 新規`src/cron.rs`に8処理それぞれを1関数として
      実装し、`main.rs`に`--cron-all`引数解釈(`std::env::args()`)を追加、
      CLIから`audiocafe-tokyo-server --cron-all`で起動できるようにする
      (PHPの`aruaru_is_cron_request()`のCGI/CLI両対応の複雑さはRustバイナリでは
      不要——Rustは常にCLI起動のみを想定すればよい)。
    - 外部クロール(楽天3種・doda・Stack Overflow/DB-Engines)は`reqwest`で
      素直に置き換え可能。正規表現抽出はPHPの`preg_match`をRustの`regex`
      クレート(既に依存に追加済み)へそのまま移植できる。
    - **最大の障壁はOpenAI API依存部分**(`aruaru_tech_apply_ai_enrichment`・
      `aruaru_learning_ai_cron_refresh`)——現行Rust側に相当するAPIキー管理・
      HTTPクライアント設定が無く、しかも「未設定ならベースライン文言」という
      フェイルセーフ込みの移植が必要。ここを後回しにし、まず楽天3種+doda
      (純粋なスクレイプ、外部AI依存なし)から着手するのが妥当。
    - 移植してもcronのスケジュール実行自体(LOLIPOPのcron設定相当)はVPS側の
      systemd timerまたはcronで別途組む必要がある(このリポジトリのコードで
      完結しない運用面の作業)。
  - **今回はここまで(実装は行っていない)**——理由: 8処理×複数の外部サイト
    スクレイプ+OpenAI連携という一括実装には検証を含め相応の時間を要し、
    「無理に完全実装しなくてよい」との指示に従い、まず正確な設計方針を
    記録した。次にすべきこと: 上記方針に沿って`src/cron.rs`を新設し、
    まず楽天3種+doda(AI依存なし)から実装・実データ検証、その後
    `ai-tech-ranking`系(OpenAI API依存)に着手。

- **2026-07-17 `index.php`本体(トップページ)のサーバー側アルゴリズムを移植・
  簡略化、`/discover`ページ新設**: `index.php`(8146行)を調査した結果、
  実質的な"PHPアルゴリズム"と呼べる部分は`is_video_host`/`source_name`/
  `extract_yt_id`/`fetch_url`/`extract_from_html`/`build_lists`
  (シードURL群からテキストリンク・動画リンク・写真を収集する仕組み、
  1日キャッシュ)の184行だけで、残り8000行超はクライアント側JavaScript
  (言語カード切替・YouTube背景プレイヤー・モーダルナビゲーション等の
  UI演出)だった。今回はこの実アルゴリズムのみを`src/scraper.rs`へ移植し、
  新規`/discover`ページとして公開した(クライアント側JS一式は対象外——
  ブラウザ側の演出はバックエンド言語に依存しないため「PHPのアルゴリズム」
  には含めない判断)。
  - **簡略化した点**: (1) PHP側は`$seen_text`/`$seen_video`/`$seen_img`
    という3つの連想配列+3回に分けたforeachで重複排除していたが、
    `HashSet`3つ+イテレータチェーンにまとめた。(2) `<a>`用・`<img>`用に
    別々に書かれていた「相対URLの絶対化」ロジックを`resolve_url()`
    1関数に集約。(3) キャッシュ永続化を[`rust_json`](https://github.com/aon-co-jp/Rust-JSON)
    経由に統一(このエコシステムのJSON処理一本化方針に合わせる)。
  - `$SEED_URLS`(360件のシードURL)は`src/seed_urls.rs`へ機械的にそのまま
    抽出・移植(取捨選択なし)。
  - **検証**: VPS上(実インターネット接続あり)で`cargo test`
    5件全green(`is_video_host`/`source_name`/`extract_yt_id`/
    `resolve_url`/`extract_from_html`の単体テスト)。実バイナリで
    `/discover`にアクセスし、実際に360件のシードURLを処理して
    動画リンク93件・記事リンク280件・写真22件を正しく収集
    (初回6秒、キャッシュヒット時9ms)——実データでの動作確認済み、
    型チェックのみでの「完了」報告ではない。`extract_yt_id`は
    YouTubeサムネイル表示(`i.ytimg.com`)に実際に使用し、未使用関数を
    残さない既存の検証基準を満たした。
  - 次にすべきこと: VPSへのsystemd反映(まだ`/root/audiocafe-tokyo-rust`の
    ビルド確認のみ、常駐サービスの再起動は次回)、クライアント側UIの
    移植要否の判断(演出目的が強く、優先度は低いと考える)。

- **2026-07-17 複合ページ(`/page/:slug`)追加、実質的な`/aruaru`・
  `/aruaru-lady`・`/rakuten-mobile`移植完了**: PHP側のこれら3パスが
  実は複数キャッシュの統合ページであることを調査で確認し、
  `COMPOSITE_PAGES`+`render_composite_body`で再現。新規に判明した
  4キャッシュ(`rakuten-smartphone`・`doda-jobs`・
  `aruaru-tvchat-normal/group-ranking`)も既存の汎用レンダラーで
  そのまま描画できることを確認(専用コード追加なし)。VPS上で実バイナリ
  再起動、`/page/aruaru`・`/page/aruaru-lady`・`/page/rakuten-mobile`
  全て200、かつ`https://audiocafe.tokyo/rust-preview/page/*`経由でも
  本番HTTPS上で200・実データ(3,278円プラン、980円国際通話等)の
  表示を確認済み。次にすべきこと: 多言語版・元デザインの再現(優先度低、
  データ表示という核心機能は完了)。
- **2026-07-17 VPSデプロイ・プレビュー公開完了**: `/root/audiocafe-tokyo-rust`
  (git管理下)でsystemd常駐化、`https://audiocafe.tokyo/rust-preview/`
  として本番PHPと並行公開。両方とも実際にHTTPS経由で200を確認済み。
  次にすべきこと: (1) `/aruaru/`等サブディレクトリページの移植、
  (2) 十分検証できたら`location /`自体の本カットオーバーを検討
  (aruaru.tokyo側の内部プロキシ依存に注意)。
- **2026-07-17 汎用レンダラーへ書き換え、全8キャッシュ対応完了**:
  形状別の専用コード(地域別/フラットrowsの2形状のみ)を、完全再帰の
  `render_value_generic`に置き換え、残り5キャッシュ
  (`ai-tech-ranking`・`aruaru-learning-prices`・`rakuten-mobile`・
  `rakuten-intl-call`・`rakuten-platinum`)も含めた全8種類で実データ検証
  済み(VPS上で実バイナリ起動、`curl`で全ルート200・実際のレンダリング
  内容を確認)。次にすべきこと: (1) `/aruaru/`等サブディレクトリページ
  自体の移植、(2) VPSへのデプロイ・カットオーバー計画(既存PHPとの同居・
  段階的切替方法の検討)。
- **2026-07-17 新規作成・第一段実装**: 上記の通り3ランキング(地域別1・
  フラット2)を実データで検証済み。
