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
