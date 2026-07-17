# audiocafe-tokyo-server

`audiocafe.tokyo`の既存PHPモノリスをRust + Poemへ段階的に移行するプロジェクト(第一段)。
既存PHP実装は[audiocafe-tokyo](https://github.com/aon-co-jp/audiocafe-tokyo)リポジトリのまま。

## 現状

`*-cache.json`(ファイルベースのジャンル別ランキングキャッシュ、
`https://audiocafe.tokyo/`直下に静的公開)をHTTP経由で取得し、
[rust-json](https://github.com/aon-co-jp/Rust-JSON)でパースして表示する。
対応済み・未対応の範囲は`CLAUDE.md`に正直に記載。

## ページ

- `/` — 対応済みランキング一覧
- `/ranking/:slug` — 個別ランキング表示(`aruaru-caba`/`aruaru-eikaiwa`/`aruaru-jukujo-caba`)
- `/healthz` — ヘルスチェック

## ビルド・起動

```bash
cargo build --release
./target/release/audiocafe-tokyo-server   # 127.0.0.1:4400
```

## ライセンス

Apache-2.0 OR MIT
