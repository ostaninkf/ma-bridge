English · [Русский](README.ru.md)

# ma-bridge — Alice → Music Assistant voice bridge

A fork of [rodyatotu/soundya](https://github.com/rodyatotu/soundya) that adds a **Music
Assistant** backend to soundya's Alice voice control. Say "Алиса, включи …" and it plays
from your own Music Assistant library, understanding Russian word cases and transliteration;
if there is no exact match it offers similar songs or a Spotify search. SoundCloud stays
available when you name it. Runs on a rooted Yandex Station as a [Luna](https://github.com/nyakokitsu/luna)
plugin (or standalone).

> Unofficial. Not affiliated with Yandex, SoundCloud, Home Assistant or Music Assistant.

## What it does

soundya's voice layer (`src/alice.rs`) taps Alice's recognized phrase on `127.0.0.1:9875` and
turns "Алиса, включи …" into an intent. This fork routes it:

| Phrase | Goes to |
|---|---|
| "Алиса, включи **Кино** / **Агату Кристи** / **белую ворону**" | **Music Assistant**, local library only (see [Local search](#local-search)) |
| "Алиса, включи **музыку**" (generic) | **Music Assistant** (resume, or start a radio/DJ stream) |
| "Алиса, включи Кино **в саундклауде**" | SoundCloud (original soundya behaviour) |
| "пауза / продолжи / что играет" | the active source (MA or SoundCloud); MA counts as active whenever its player is playing (checked every 15 s) |
| "дальше / назад" | **always MA**, unless the bridge's own SoundCloud is playing |
| "… **в яндекс музыке**" | native Alice (unchanged) |

The bridge does not stream audio itself — it calls the **Home Assistant REST API**
(`music_assistant.play_media` + `media_player.*`), and Music Assistant plays on the speaker's
own player entity through its provider. An explicit "включи X" plays exactly that, without
`radio_mode`; the endless "DJ" stream is used only for "something similar / recommend".

MA intents implemented: play (artist/track/album/playlist), pause/resume/next/previous, seek,
shuffle, repeat, volume, "what's playing", "similar/recommend" (radio of the current artist),
and generic "включи музыку" (resume; never starts a DJ stream).

## Local search

"Включи X", "включи альбом X", "включи плейлист X" search **only the local library** of
Music Assistant. Music Assistant's own search matches substrings, which fails on spoken
Russian ("включи Агутина" → "агутина" is not a substring of "Агутин"), so the bridge keeps its
own index and matches itself (`src/local.rs`):

- **Index** — `music_assistant.get_library` for artists, albums, tracks and playlists
  (~3.5 s for 2,700 items), kept in memory, rebuilt in the background every 15 min and on a
  miss if older than 5 min. Only what has tracks counts as local: artists/albums imported from
  streaming services without local tracks are skipped, as are MA's generated playlists and
  playlists of other services.
- **Matching** — both sides transliterated to Latin; a word matches exactly, in another
  grammatical case (same stem, both endings from a list of Russian endings), or — Cyrillic
  against Latin only — by consonant skeleton ("квин" → Queen, "элтона джона" → Elton John,
  "битлз" → The Beatles). Initials match by first letter ("булата окуджаву" → "Песни Б.
  Окуджавы"); Ukrainian letters are folded to Russian ("иконы не плачут" → "Ікони не
  плачуть"). A leading "песню / альбом / группу" is a type hint and is not lost from titles
  like "Группа крови".
- **Exact match** — plays at once.
- **Nothing in common** — Alice says "Локально не найдено: X".
- **No exact match, but similar** (tracks whose title shares a word with the request, exact or
  in another case) — Alice asks *"Точно такого нет. Нашла похожие песни: N. Включить их или
  поискать на Спотифай?"* and the speaker **starts listening right away** (no wake word needed,
  10 s): "включай / да" plays the similar songs, "на Спотифай" searches Spotify (artists, tracks,
  albums chosen by the same strict matching: artist > track > album), "нет" pauses silently.
  Alice's own reply to the answer is cut off.

Listening without the wake word uses `AliceCapability` method **2 (startConversation)**. The
method numbers are not published; they were recovered read-only from the station's `maind`
binary (lambda typeinfo → libc++ `std::function` policy → invoker constant) and checked
against the known ones: 0 startRequest, 1 cancelDialog, 2 startConversation,
3 stopConversation, 4 toggleConversation, 5 finishConversationVoiceInput,
6 cancelDialogAndClearQueue, 14 stopTts, 18 startVocalize, 19 startRecognize.

A voice request plays once: the bridge turns the queue's repeat off before starting it (what
happens when it ends is up to Home Assistant — e.g. go back to a DJ playlist). "Пауза" remembers
the track and second; "продолжи" seeks back there if Music Assistant restarts the track from 0:00
(it does in flow mode when Alice speaks during the pause).

Diagnostics without playing anything: `POST http://<speaker>:8765/api/cmd` with body
`find <query>` returns what the search would play (`OK …`, `SIMILAR …` or `NOT FOUND …`).

## TV

With a TV/projector set up, "Алиса, найди в пубере / в кинопабе X на телевизоре" opens a search
in the Puber (kino.pub) Android TV app via `puber://search?q=X`.

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

## Changes vs. soundya

- `src/ma.rs` — **new**: Home Assistant REST client, local index, similar-songs question,
  Spotify search.
- `src/local.rs` — **new**: matching of spoken requests against the library (cases,
  transliteration, consonant skeleton, initials) and similar songs.
- `src/alice.rs` — routing (music → MA by default, "саундклауд" → SoundCloud; transport follows
  the active source; `keyword_only` skipped while MA is on; generic "включи музыку" → MA).
- `src/main.rs` — `Cmd::Voice(Intent, bool)`, `ma_intent`, `Shared.ma_enabled/ma_active`.
- `src/settings.rs` — `ma_enabled/ha_url/ha_token/ma_entity`.
- `src/web.html` — Music Assistant settings card.
- `src/{buttons,luna}.rs` — buttons and Luna commands follow the active source.
- `src/tv.rs` — "найди в пубере X" → `puber://search` deep link.
- Alice speech events: accepted both with envelope tag 253=3 and 253=210 (newer firmware),
  otherwise the end of speech was never seen and every `say` waited 15 s.

Everything soundya already did (SoundCloud by voice, AirPlay, PC/TV, bypass) is unchanged.

## Credits & license

MIT, same as upstream. Built on **soundya** by [rodyatotu](https://github.com/rodyatotu/soundya),
which is based on **spotiya** by [klockky](https://github.com/klockky/spotiya). See
[LICENSE](LICENSE) and [docs/README.upstream-soundya.md](docs/README.upstream-soundya.md).
