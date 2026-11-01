English · [Русский](README.ru.md)

# ma-bridge — Alice → Music Assistant voice bridge

A fork of [rodyatotu/soundya](https://github.com/rodyatotu/soundya) that adds a **Music
Assistant** backend to soundya's Alice voice control. Say "Алиса, включи …" and it plays
through Music Assistant (Yandex Music, Spotify, your own library); SoundCloud stays available
when you name it. Runs on a rooted Yandex Station as a [Luna](https://github.com/nyakokitsu/luna)
plugin (or standalone).

> Unofficial. Not affiliated with Yandex, SoundCloud, Home Assistant or Music Assistant.

## What it does

soundya's voice layer (`src/alice.rs`) taps Alice's recognized phrase on `127.0.0.1:9875` and
turns "Алиса, включи …" into an intent. This fork routes it:

| Phrase | Goes to |
|---|---|
| "Алиса, включи **Кино**" | **Music Assistant** |
| "Алиса, включи **музыку**" (generic) | **Music Assistant** (resume, or start a radio/DJ stream) |
| "Алиса, включи Кино **в саундклауде**" | SoundCloud (original soundya behaviour) |
| "пауза / дальше / что играет / громче" | the active source (MA or SoundCloud) |
| "… **в яндекс музыке**" | native Alice (unchanged) |

The bridge does not stream audio itself — it calls the **Home Assistant REST API**
(`music_assistant.play_media` + `media_player.*`), and Music Assistant plays on the speaker's
own player entity through its provider. `radio_mode` (endless "DJ" stream) is on by default.

MA intents implemented: play (artist/track/album/playlist), pause/resume/next/previous, seek,
shuffle, repeat, volume, "what's playing", "similar/recommend" (radio of the current artist),
and generic "включи музыку" (resume, else radio from the last track).

## Requirements

- A Yandex Station with root + ADB and [Luna](https://github.com/nyakokitsu/luna) installed via
  [YAM](https://github.com/nyakokitsu/yam) (tested on Station Mini 2, `yandexmini_2`).
- Home Assistant with the Music Assistant integration and a `media_player` entity for the
  speaker, plus a long-lived access token.

## Build

Needs Rust (rustup). Produces artifacts in `dist/`.

```sh
sh scripts/build-arm.sh          # armv7 (Linaro, glibc ≤2.23) -> dist/soundya-armv7.tar

# Luna package (.yampkg):
git clone https://github.com/nyakokitsu/yam ../yam
(cd ../yam && cargo build --release --bin yam-publish)
YAM_SRC=../yam YAM_PACK_ONLY=1 YAM_TARGETS=yandexmini_2 sh scripts/yam-release.sh
```

## Install (as a Luna package)

```sh
adb push dist/soundya-<ver>-yandexmini_2.yampkg /tmp/ma.yampkg
# Luna holds the YAM lock, so stop it during install:
ssh root@<speaker> '/etc/init.d/S99luna stop && \
  /data/yam/bin/yam-run install --yes /tmp/ma.yampkg && \
  /etc/init.d/S99luna start'
```

Two soundya instances cannot run at once (they share the voice tap, port 8765 and AirPlay) —
install this instead of the stock soundya. The standalone (non-Luna) path from upstream
(`scripts/deploy.sh`) also works; see [docs/README.upstream-soundya.md](docs/README.upstream-soundya.md).

## Configure Music Assistant

Open `http://<speaker>:8765/` → **Music Assistant**:

- **Home Assistant URL** — e.g. `http://homeassistant.local:8123`
- **Player entity** — the speaker's `media_player.*` in MA/HA
- **Token** — a Home Assistant long-lived access token (stored only on the speaker, masked in
  the UI)
- **Default search type** and **DJ (radio_mode)** as you like

## Changes vs. soundya

- `src/ma.rs` — **new**: Home Assistant REST client.
- `src/alice.rs` — routing (music → MA by default, "саундклауд" → SoundCloud; transport follows
  the active source; `keyword_only` skipped while MA is on; generic "включи музыку" → MA).
- `src/main.rs` — `Cmd::Voice(Intent, bool)`, `ma_intent`, `Shared.ma_enabled/ma_active`.
- `src/settings.rs` — `ma_enabled/ha_url/ha_token/ma_entity/ma_radio/ma_type`.
- `src/web.html` — Music Assistant settings card.
- `src/{buttons,luna}.rs` — buttons and Luna commands follow the active source.

Everything soundya already did (SoundCloud by voice, AirPlay, PC/TV, bypass) is unchanged.

## Credits & license

MIT, same as upstream. Built on **soundya** by [rodyatotu](https://github.com/rodyatotu/soundya),
which is based on **spotiya** by [klockky](https://github.com/klockky/spotiya). See
[LICENSE](LICENSE) and [docs/README.upstream-soundya.md](docs/README.upstream-soundya.md).
