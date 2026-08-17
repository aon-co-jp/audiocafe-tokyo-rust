# audiocafe-tokyo-server

A project to gradually migrate `audiocafe.tokyo`'s existing PHP monolith to Rust + Poem
(phase one). The existing PHP implementation continues to live in the
[audiocafe-tokyo](https://github.com/aon-co-jp/audiocafe-tokyo) repository.

## Current state

Fetches `*-cache.json` (a file-based, per-genre ranking cache published statically at the
root of `https://audiocafe.tokyo/`) over HTTP and parses it with
[rust-json](https://github.com/aon-co-jp/Rust-JSON) for display. What is and isn't yet
covered is disclosed honestly in `CLAUDE.md`.

## Pages

- `/` — the ranking list currently supported, plus the YouTube playlist "series" feature
  (a port of the PHP version's `SEARCH_SERIES`, stored in `assets/search_series.json`)
- `/ranking/:slug` — a single ranking view (`aruaru-caba`/`aruaru-eikaiwa`/`aruaru-jukujo-caba`)
- `/healthz` — health check

## Editing the YouTube playlist series (important)

`assets/search_series.json` is embedded at compile time via `include_str!`, so **editing and
pushing it alone does NOT take effect in production**. You must `git pull` → `cargo build
--release` → `systemctl restart audiocafe-tokyo-rust` on the VPS and verify over real HTTP.
For data-design caveats (e.g. link behavior for series with no video), see the relevant
section of `PORTING.md`.

## Build & run

```bash
cargo build --release
./target/release/audiocafe-tokyo-server   # 127.0.0.1:4400
```

## License

Apache-2.0 OR MIT
