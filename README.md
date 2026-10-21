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
  The play button on a poster (and **Watch now**) starts playback right away, continuing where
  you left off. Clicking the rest of the poster opens the details page.
- **Details**: a backdrop, season tabs, watched and upcoming episodes, one-click Play/Continue, and
  a stream list sorted by seeders, with quality filters, size and source.
- **Autoplay**: picking an episode (or Play) starts the best source right away. The order is 1080p
  first, then 720p, 4K and SD, and the most seeders within each quality. When you continue a
  video, the source you watched it from is used again (it's already in the cache). If a source fails, use
  **Try the next source**, or close the player and pick one from the list.
- **Subtitles**: Romanian is picked automatically, from the file if it has a Romanian track and
  otherwise from [OpenSubtitles](https://www.opensubtitles.org/) (or any installed subtitles addon).
  All languages are in the player's subtitles menu. Z / X shift the timing, and legacy
  Windows-1250 files are decoded correctly. Your last pick is remembered: the same video gets
  the same track back, and the next episode gets the same language (or stays off).
- **Torrent streaming** with [librqbit](https://github.com/ikatson/rqbit). It supports HTTP range
  requests and seeking, and downloads only the selected file.
- **2 GB cache**: what you've watched stays on disk, together with the torrent metadata. Replaying
  (even after a restart) starts instantly, with no waiting for peers or downloading again. The
  least recently played torrents are removed once the cache is full, and the one you're watching
  is always kept. Set `KINORA_CACHE_GB` to change the size. The cache is in
  `~/Library/Caches/kinora/` (macOS) or `~/.cache/kinora/`.
- **Embedded player** (FFmpeg + cpal) for any codec FFmpeg supports (HEVC, AC3, DTS…), with seeking,
  audio track and subtitle selection, full screen, resume, and "up next". The display stays on
  while a video plays (macOS), and can sleep again when you pause. An external player is still one click away.
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
| Z / X | Subtitles earlier / later |
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
- `src/subtitles.rs`: subtitles from addons, SRT/WebVTT parsing, encoding detection
- `src/library.rs`, `src/player.rs`: library persistence and external player launch
- `src/bin/desktop/`: the desktop app (egui), with `main.rs` (shell, navigation), `pages.rs`,
  `detail.rs`, `playback.rs` (player UI), `kit.rs` (theme, icons, widgets) and `video.rs` (FFmpeg player)
- `src/bin/web.rs` + `static/index.html`: the browser version (vanilla JS, embedded in the binary)
- `docs/`: the project website (GitHub Pages)

## Privacy & security

The HTTP server listens only on `127.0.0.1` and rejects requests for any other `Host` header, which
blocks DNS rebinding. Stream URLs from addons must be `http(s)`. Cached torrent data stays in the
cache directory above, and you can delete it at any time.

## Disclaimer

Kinora is an independent project and is not affiliated with Stremio. It ships no content. You are
responsible for the addons you install and for what you stream in your jurisdiction.

## License

[MIT](LICENSE)
