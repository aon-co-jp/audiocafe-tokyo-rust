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

## HANDOFF

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
