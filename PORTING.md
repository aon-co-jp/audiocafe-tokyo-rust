# PORTING.md — audiocafe-tokyo-rust お引越しファイル

> このファイル1枚で、他プロジェクトへ `audiocafe-tokyo-rust` を導入・移設できます。
> 対象バージョン: 0.3.0(2026-07-19、国旗クリック導線・翻訳リンク・
> YouTubeシリーズ切替を追加——詳細は`CLAUDE.md`のHANDOFFログ参照)。

## 0. このリポジトリのスコープ

`audiocafe-tokyo-rust`(旧フォルダ名 `audiocafe-tokyo-server`)は、
既存PHPモノリス [`audiocafe-tokyo`](https://github.com/aon-co-jp/audiocafe-tokyo)
(`index.php`単体445KB・189関数・8146行)をRust + **RPoem**
(`open-runo-router::hyper_compat`、外部`poem`クレートへの直接依存を
断つエコシステム方針、2026-07-19に移行完了)へ段階的に移行する
リポジトリ。既存PHP実装は上書きせず、独立した移行先として運用している。

**2026-07-19時点の到達度**:
トップページ(`/`)・`/aruaru`・`/aruaru-lady`・`/rakuten-mobile`の
**4ページ**は、PHP版と**内容+見た目(CSS/クラス構造)の両方が一致**
するレベルまで移植済みで、**本番`audiocafe.tokyo`ドメインで実際に
Rust版が稼働中**(nginx `location = /`および`location /aruaru/`等の
個別パスでプロキシ)。トップページは147/147言語カード全件・各カードの
全文エッセイ(政治・宗教等の主張を含め実際に公開済みの内容をそのまま
複製)・`cardLinks`・YouTube背景プレイヤー(**77件のシリーズ切替+
NEXT+OPEN/CLOSEパネル、再実装済み**)・無料壁紙コーナーまで含む。
さらに国旗画像自体がクリック可能(Google翻訳プロキシ経由でaudiocafe.tokyo
本体へ)、`.card-actions`の`/aruaru`・`/aruaru-lady`・`/rakuten-mobile`
リンクも選択言語のGoogle翻訳プロキシ経由になった(2026-07-19)。
このRustサイトは従来「クライアント側JSを持たない」方針だったが、
YouTubeシリーズ切替機能に限り小さな自己完結スクリプトを初めて導入した
(詳細・スコープ外事項は`CLAUDE.md`のHANDOFFログ2026-07-19エントリ参照)。

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

## 5. まだ移植していないもの(ごまかさず明記、2026-07-19更新)

- **PHP版の元の遷移先選択モーダル(`#acNavChoiceModal`)自体のJS
  アニメーション**: 到達できる行き先(audiocafe.tokyo本体/aruaru/
  aruaru-lady/rakuten-mobile/aruaru.tokyo/Google Translate、いずれも
  国旗画像・`.card-actions`双方から選択言語のGoogle翻訳プロキシ経由で
  到達可能)は同一だが、モーダルの開閉演出自体は静的直リンクのまま。
- **YouTube再生リストのシリーズ機能(`SEARCH_SERIES`、実際は77件——
  旧HANDOFFの「84件」という記載は未検証の見積もりで、Node.jsでの
  lossless評価により77件と訂正)**: 2026-07-19に復活済み。77件全ての
  シリーズボタン・NEXTでのキュー送り・OPEN/CLOSEパネル切替を実装
  (このRustサイト初のクライアント側JS、小さな自己完結スクリプト)。
  ただし、PHP版が持つYouTube IFrame API連携(自動キュー送り・
  シャッフル・検索駆動の動画切替アニメーション)は対象外——
  NEXTボタンでの手動送りのみ。非再生可能なシリーズ(`results?
  search_query=`のみ等)は実際のYouTube検索結果ページへ新規タブで
  遷移(`audiocafe.tokyo/CLAUDE.md`のスクレイプ禁止方針を踏襲)。
- 多言語版(`index-en.php`・`index-fr.php`等、`/aruaru/`だけで12言語)— 日本語版相当のみ。トップページの言語カード経由でのGoogle翻訳プロキシ閲覧は可能。
- **`/cancer`・`/Python`・`/video`・`/world`(削除予定)ディレクトリ**:
  静的コンテンツ・配布ツールファイルで、Rust側では未移植のままnginxの
  静的配信(PHP版と同じドキュメントルート)に依存している。
- クライアント側JavaScriptの演出のうち、上記のYouTubeシリーズ切替
  以外の細部(検索駆動の動画切替アニメーション、シャッフル等)。

## 6. 本番デプロイ(2026-07-19、カットオーバー実施済み)

VPS上 `/root/audiocafe-tokyo-rust`(GitHubからclone、git管理下)で
`cargo build --release`、systemdサービス化
(`audiocafe-tokyo-rust.service`、`127.0.0.1:4400`)。

`/etc/nginx/conf.d/audiocafe.tokyo.conf`に以下を追加し、**本番
`https://audiocafe.tokyo/`で実際にRust版が稼働中**:
- `location = /`(完全一致のみ、prefix matchより優先)→ `127.0.0.1:4400/`
- `location /aruaru/`・`/aruaru-lady/`・`/rakuten-mobile/`
  (各prefix match)→ `127.0.0.1:4400/aruaru`等

これら以外の全パス(`location /`のprefix match、`/top/`・`/cancer/`・
`/Python/`・`/video/`・静的キャッシュJSON等)は**無変更のままPHP側が
処理を継続**——`location =`は完全一致のみを奪うnginxの通常規則を
利用しているため、既存コンテンツへの影響は無い。設定変更前は必ず
`cp audiocafe.tokyo.conf audiocafe.tokyo.conf.bak-<timestamp>`で
バックアップを取ってから編集すること。

他環境へ移設する場合は、まず対象ドメインの実際のドキュメントルート
構成(このドメイン固有の`/top/`等の個人情報・配布物ディレクトリの
有無)を調査してから、同じ「完全一致のみを奪う」パターンで安全に
段階的カットオーバーすること——`location /`を丸ごと置き換えると、
Rust側に実装のないパスが軒並み404になる実害があることを確認済み
(`CLAUDE.md`のHANDOFFログ2026-07-19参照)。

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
