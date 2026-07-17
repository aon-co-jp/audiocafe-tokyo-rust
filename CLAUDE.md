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

**まだ移植していないもの(ごまかさず明記)**:
- PHP側の`/aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`サブディレクトリ
  ページ自体(HTML構造、静的画像/動画配信)——今回は「キャッシュJSONの
  データ表示」だけをカバーした段階。
- 元のPHPページのデザイン・レイアウトは再現していない(機能等価の
  最小限HTMLのみ)。

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
