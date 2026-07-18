# PORTING.md — audiocafe-tokyo-rust お引越しファイル

> このファイル1枚で、他プロジェクトへ `audiocafe-tokyo-rust` を導入・移設できます。
> 対象バージョン: 0.1.0(2026-07-17、`audiocafe.tokyo` PHPモノリスからの移行 第一段)。

## 0. このリポジトリのスコープ

`audiocafe-tokyo-rust`(旧フォルダ名 `audiocafe-tokyo-server`)は、
既存PHPモノリス [`audiocafe-tokyo`](https://github.com/aon-co-jp/audiocafe-tokyo)
(`index.php`単体445KB・189関数・8146行)をRust + Poemへ段階的に
移行するための新規リポジトリ。既存PHP実装は上書きせず、独立した
移行先として運用している。

**移植方針(正直な開示)**: PHP側の8000行超の大半は装飾目的の
クライアント側JavaScript(言語カード切替・YouTube背景プレイヤー・
モーダルナビゲーション等)であり、"アルゴリズム"と呼べる実質的な
処理は少数の関数(`*-cache.json`のジャンル別ランキング表示、
`/discover`ページの動画・記事・写真収集ロジック計184行)に
限られていた。これらの実アルゴリズムのみを簡略化しつつ移植し、
装飾目的のUIコードは対象外とした。

## 1. 持っていくもの(ファイル一覧)

```
audiocafe-tokyo-rust/
├── Cargo.toml / Cargo.lock
├── src/
│   ├── main.rs        # ルーティング・汎用レンダラー(render_value_generic)・複合ページ
│   ├── scraper.rs      # /discover 用スクレイピングロジック(旧PHPの実アルゴリズム部分)
│   └── seed_urls.rs    # シードURL360件(機械的にそのまま移植)
├── PORTING.md(本ファイル)
├── CLAUDE.md
└── README.md
```

丸ごと移設する場合はフォルダごとコピーして `cargo build --release` が
通れば移設成功。

## 2. ビルド・起動

```bash
cargo build --release
./target/release/audiocafe-tokyo-server   # 既定バインド 127.0.0.1:4400
```

## 3. ページ構成

- `/` — 対応済みランキング一覧
- `/ranking/:slug` — 個別ランキング表示(`aruaru-caba`/`aruaru-eikaiwa`/`aruaru-jukujo-caba`等、全8種)
- `/page/:slug` — 複合ページ(`aruaru`/`aruaru-lady`/`rakuten-mobile`、複数キャッシュを1ページに統合)
- `/discover` — 動画・記事・写真収集ページ(`src/scraper.rs`、シードURL360件から収集)
- `/healthz` — ヘルスチェック

## 4. データ取得方式

`*-cache.json`(PHP側がファイルベースで生成し `https://audiocafe.tokyo/`
直下に静的公開しているジャンル別ランキングキャッシュ)を**HTTP経由で
取得**し(サーバー間の直接ファイル共有を前提にしない疎結合設計)、
[`rust_json::parse_strict`](https://github.com/aon-co-jp/Rust-JSON)で
パースして `render_value_generic`(`src/main.rs`)で汎用的にレンダリングする。
キャッシュのスキーマは8種類あるが、形状ごとの専用コードは書かず、
完全再帰の汎用レンダラー1つで全種類をカバーしている。

移設先で別のキャッシュJSON形状が増えても、既存の`render_value_generic`
がそのまま通用するかをまず確認し、専用コードの追加は最終手段とすること。

## 5. まだ移植していないもの(ごまかさず明記)

- 元のPHPページのHTML構造・デザイン・レイアウト(装飾・画像配置等)
- 多言語版(`index-en.php`・`index-fr.php`等、`/aruaru/`だけで12言語)— 日本語版相当のみ
- **cronによるキャッシュ自動更新ロジック自体**(2026-07-18調査済み、未実装) —
  Rust側は既存キャッシュを読むだけで、更新は引き続きPHP側のcronに依存。
  実体は`audiocafe.tokyo/aruaru/index.php`(7514〜7649行目)の`--cron-all`
  統合ブロックで、8処理(技術ランキング同期・学習価格・AI学習コメント・
  英会話ランキング・楽天モバイル基本料金/国際通話/プラチナバンド・doda求人)
  を毎日実行し、各々が外部サイトへの実クロールまたはOpenAI API呼び出しを
  伴う。調査結果と移植方針(`src/cron.rs`新設案、楽天3種+dodaから着手し
  OpenAI依存の技術ランキングは後回し)は`CLAUDE.md`のHANDOFFログ
  (2026-07-18)に記録済み。スコープが大きいため実装は次回以降。
- クライアント側JavaScriptの演出(言語カード切替・YouTube背景プレイヤー・モーダルナビゲーション)
- 本番カットオーバー(`location /`自体をRust版に切り替える)— `aruaru.tokyo`が内部プロキシで`/aruaru/`等のパスに依存しているため未実施

## 6. デプロイ(現状)

VPS上 `/root/audiocafe-tokyo-rust`(GitHubからclone、git管理下)で
`cargo build --release`、systemdサービス化
(`audiocafe-tokyo-rust.service`、`127.0.0.1:4400`)。既存PHP本番
(`https://audiocafe.tokyo/`)は一切変更せず、`audiocafe.tokyo.conf`の
443番serverブロックに `location /rust-preview/` のみ追加してプロキシする
形で並行公開している。他環境へ移設する場合も、本番の`location /`は
最後まで変更しないこと。

## 7. 命名規約

- クレート名・バイナリ名: `audiocafe-tokyo-server`(実行ファイル名は移行時のまま据え置き)
- リポジトリ名・フォルダ名: `audiocafe-tokyo-rust`(2026-07-18、ローカルフォルダ名をリモートリポジトリ名に合わせて改名)

## 8. 移植・拡張時の注意

新しいキャッシュJSONの形状が増えた場合は、まず既存の汎用レンダラー
(`render_value_generic`)で描画できるかを試すこと。形状別の専用コードを
安易に追加すると、8種類対応時に一度解消した「形状ごとの分岐地獄」に
逆戻りする。技術選定・PHP側アルゴリズムの解釈で迷う場合は、
学習データからの推測のみに頼らず、実際のVPS上のPHPソース・実データを
確認してから移植すること。
