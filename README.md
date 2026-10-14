# audiocafe-tokyo-server

`audiocafe.tokyo`の既存PHPモノリスをRust + Poemへ段階的に移行するプロジェクト(第一段)。
既存PHP実装は[audiocafe-tokyo](https://github.com/aon-co-jp/audiocafe-tokyo)リポジトリのまま。

## 現状

`*-cache.json`(ファイルベースのジャンル別ランキングキャッシュ、
`https://audiocafe.tokyo/`直下に静的公開)をHTTP経由で取得し、
[rust-json](https://github.com/aon-co-jp/Rust-JSON)でパースして表示する。
対応済み・未対応の範囲は`CLAUDE.md`に正直に記載。

## ページ

- `/` — 対応済みランキング一覧、およびYouTube再生リストシリーズ機能
  (PHP版`SEARCH_SERIES`の移植、`assets/search_series.json`)
- `/ranking/:slug` — 個別ランキング表示(`aruaru-caba`/`aruaru-eikaiwa`/`aruaru-jukujo-caba`)
- `/healthz` — ヘルスチェック

## YouTube再生リストシリーズの編集について(重要)

`assets/search_series.json`は`include_str!`でコンパイル時に埋め込まれる
ため、**編集してpushしただけでは本番に反映されない**。必ずVPS側で
`git pull` → `cargo build --release` → `systemctl restart
audiocafe-tokyo-rust`まで行い、実HTTPで反映を確認すること。詳細な
データ設計上の注意点(動画を含まないシリーズのリンク挙動等)は
`PORTING.md`の該当節を参照。

## ビルド・起動

```bash
cargo build --release
./target/release/audiocafe-tokyo-server   # 127.0.0.1:4400
```

## ライセンス

Apache-2.0 OR MIT

## オンライン学習サイト・AI駆動開発の研究記事の掲載と引越し(2026-10-08/14)
`/aruaru`の一番上と「おすすめ学習サービスTOP50」の冒頭に、オンライン学習サイト5件と研究記事(`ONLINE_LEARNING_LIST_HTML` / `AI_DRIVEN_RESEARCH_HTML`)を追加。本番で表示確認済み(`aruaru.tokyo/aruaru/`のミラーにも出る)。
**未了**: AIが作った学習リンク(`aruaru/aruaru-tech-learning-ai-cache.json`、80技術・約770件、価格欄は訂正済み)のRust版への表示。
**お引越し**: 本番は`/root/audiocafe-tokyo-rust`で`cargo build --release`→`systemctl restart audiocafe-tokyo-rust.service`(ポート4400)。キャッシュJSONは`/var/www/audiocafe.tokyo`から読むため、PHP側の毎朝クロールが先に必要。
手順全体・systemd・nginxの設定は`audiocafe-tokyo`の`PORTING.md`と`deploy/vps/`。
