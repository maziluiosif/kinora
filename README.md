# Kinora

A fast, native media center written in Rust. Kinora browses catalogs from addons that speak the
[Stremio addon protocol](https://github.com/Stremio/stremio-addon-sdk/blob/master/docs/protocol.md),
streams torrents with a built-in engine, and plays them in an embedded FFmpeg player. Everything
runs in one process on your machine, with no account and no cloud.

**Website:** https://maziluiosif.github.io/kinora/

## Quick start

Requires Rust (1.88 or newer) and the FFmpeg libraries.

```sh
brew install ffmpeg pkg-config      # macOS
cargo run --release                 # desktop app (the default)
cargo run --release --bin kinora-web               # browser UI at http://127.0.0.1:11471/
cargo run --release --bin kinora-web -- --no-open  # without opening the browser
```

On Linux, install the FFmpeg development packages (for example `libavformat-dev libavcodec-dev
libswscale-dev libswresample-dev libavdevice-dev libavfilter-dev pkg-config libclang-dev` on Debian/Ubuntu).

## Features

- **Addons**: Cinemeta (catalogs and metadata) and Torrentio (streams) are preinstalled. Torrentio
  is configured with `language=romanian|qualityfilter=cam,scr,threed`. You can install or remove
  any other addon by its `manifest.json` URL from the Addons page.
- **Board** with every addon catalog, **Discover** with infinite scroll, and **Search** across all addons.
- **Details**: a backdrop, season tabs, watched and upcoming episodes, one-click Play/Continue, and
  a stream list with quality filters, seeders, size and source.
- **Torrent streaming** with [librqbit](https://github.com/ikatson/rqbit). It supports HTTP range
  requests and seeking, downloads only the selected file, and deletes the previous torrent when you switch.
- **Embedded player** (FFmpeg + cpal) for any codec FFmpeg supports (HEVC, AC3, DTS…), with seeking,
  audio track and subtitle selection (Romanian or forced subtitles are picked automatically), full
  screen, resume, and "up next". An external player is still one click away.
- **Library** and **Continue watching**, stored in `~/.config/kinora/`.

### Keyboard shortcuts (desktop)

| Keys | Action |
| --- | --- |
| ⌘F or `/` | Search |
| ⌘[ · Option ← · Backspace | Back |
| Space | Play / pause |
| ← / → | Seek 10 s |
| ↑ / ↓ | Volume |
| F · double-click | Full screen |
| M | Mute |
| N | Next episode |
| Esc | Leave full screen / close the player |
| ? | All shortcuts |

### External player

The web UI (and the desktop's "external player" button) uses `$KINORA_PLAYER` if it is set.
Otherwise it tries mpv, IINA, VLC and ffplay, in that order. In-browser playback only works for
mp4/x264; MKV/HEVC/AC3 need the desktop app or an external player.

## Layout

- `src/lib.rs`: `Core`, the shared logic (catalogs, search, meta, streams, play)
- `src/addons.rs`: manifests, resource matching, catalog/meta/stream requests
- `src/engine.rs`: torrent engine (magnet → file selection → stream)
- `src/server.rs`: HTTP API and the `/stream/{torrent}/{file}/{name}` endpoint
- `src/library.rs`, `src/player.rs`: library persistence and external player launch
- `src/bin/desktop/`: the desktop app (egui), with `main.rs` (shell, navigation), `pages.rs`,
  `detail.rs`, `playback.rs` (player UI), `kit.rs` (theme, icons, widgets) and `video.rs` (FFmpeg player)
- `src/bin/web.rs` + `static/index.html`: the browser version (vanilla JS, embedded in the binary)
- `docs/`: the project website (GitHub Pages)

## Privacy & security

The HTTP server listens only on `127.0.0.1` and rejects requests for any other `Host` header, which
blocks DNS rebinding. Stream URLs from addons must be `http(s)`. Torrent data is temporary and lives
in `$TMPDIR/kinora*`.

## Disclaimer

Kinora is an independent project and is not affiliated with Stremio. It ships no content. You are
responsible for the addons you install and for what you stream in your jurisdiction.

## License

[MIT](LICENSE)
