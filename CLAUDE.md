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
汎用的にレンダリングする(`src/main.rs`の`render_ranking_body`)。

キャッシュのスキーマは8種類あるが完全には統一されていない。今回**対応済み**
なのは以下の2形状・3ランキングのみ:

- **地域別形状**(`tokyo_23`/`tokyo_tama`/`national`、各`{rows:[...]}`):
  `aruaru-caba-ranking-cache.json`
- **フラット`rows`形状**: `aruaru-eikaiwa-ranking-cache.json`・
  `aruaru-jukujo-caba-ranking-cache.json`

**未対応(次回以降の移行対象、ごまかさず明記)**:
- `ai-tech-ranking-cache.json`(`databases`/`frameworks`/`languages`/
  `ranking_meta`という異なるネスト構造)
- `aruaru-learning-prices-cache.json`(`dotinstall_monthly`/
  `paiza_monthly`/`progate_monthly`/`sources`)
- `rakuten-intl-call-cache.json`・`rakuten-platinum-cache.json`
  (日英併記フィールド、`crawl_success`/`crawled_at`等クロール系メタ情報)
- `rakuten-mobile-cache.json`(`plan`/`price`/`source_url`の単純な形だが
  今回は未実装)
- PHP側の`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`サブディレクトリ
  ページ自体(HTML構造、静的画像/動画配信)はまだ移植していない——今回は
  「ランキングデータの表示」だけを検証した段階。
- 元のPHPページのデザイン・レイアウトは再現していない(機能等価の
  最小限HTMLのみ)。

## 検証(2026-07-17)

VPS上(実インターネット接続あり)で`cargo build`→実バイナリ起動→
`curl`で`/`・`/ranking/aruaru-caba`・`/ranking/aruaru-eikaiwa`・
`/ranking/aruaru-jukujo-caba`すべて200、かつ実際に本番の
`https://audiocafe.tokyo/*-cache.json`を取得してランキング表が
正しく描画されることを確認済み(型チェックのみでの「完了」報告ではない)。

## デプロイ(未実施)

まだVPSへのsystemd常駐化・nginx vhost設定は行っていない
(既存PHP側と同居させる必要があり、カットオーバー方法の検討が必要)。
既存`audiocafe.tokyo`のnginx vhostは、`aruaru.tokyo`から
`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`が内部プロキシで参照
している実体でもあるため、これらのパスの扱いを崩さないよう
カットオーバー計画を別途立てる必要がある。

## 関連プロジェクト

- [audiocafe-tokyo](https://github.com/aon-co-jp/audiocafe-tokyo) — 既存PHP実装(移行元)
- [Rust-JSON](https://github.com/aon-co-jp/Rust-JSON) — JSON解析に使用
- [aruaru-tokyo-server](https://github.com/aon-co-jp/aruaru-tokyo-server) — 技術スタックの出典元

## HANDOFF

- **2026-07-17 新規作成・第一段実装**: 上記の通り3ランキング(地域別1・
  フラット2)を実データで検証済み。次にすべきこと: (1) 残り5キャッシュの
  対応、(2) `/aruaru/`等サブディレクトリページ自体の移植、(3) VPSへの
  デプロイ・カットオーバー計画(既存PHPとの同居・段階的切替方法の検討)。
