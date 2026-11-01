[English](README.md) · Русский

# ma-bridge — голосовой мост Алиса → Music Assistant

Форк [rodyatotu/soundya](https://github.com/rodyatotu/soundya): к голосовому управлению soundya
добавлен бэкенд **Music Assistant**. Говоришь «Алиса, включи …» — играет через Music Assistant
(Яндекс.Музыка, Spotify, своя библиотека); SoundCloud остаётся, если его назвать. Работает на
рутованной Яндекс-Станции как плагин [Luna](https://github.com/nyakokitsu/luna) (или автономно).

> Неофициальный проект. Не связан с Яндексом, SoundCloud, Home Assistant и Music Assistant.

## Что делает

Голосовой слой soundya (`src/alice.rs`) перехватывает распознанную Алисой фразу на
`127.0.0.1:9875` и превращает «Алиса, включи …» в интент. Форк его маршрутизирует:

| Фраза | Куда |
|---|---|
| «Алиса, включи **Кино**» | **Music Assistant** |
| «Алиса, включи **музыку**» (генерик) | **Music Assistant** (продолжить или запустить волну/диджей) |
| «Алиса, включи Кино **в саундклауде**» | SoundCloud (как в оригинале) |
| «пауза / дальше / что играет / громче» | активный источник (MA или SoundCloud) |
| «… **в яндекс музыке**» | родная Алиса (без изменений) |

Мост сам звук не тянет — он зовёт **REST API Home Assistant** (`music_assistant.play_media` +
`media_player.*`), а Music Assistant играет на плеере-колонке своим провайдером. `radio_mode`
(бесконечный «диджей»-поток) включён по умолчанию.

Реализовано для MA: воспроизведение (исполнитель/трек/альбом/плейлист),
пауза/продолжить/дальше/назад, перемотка, перемешивание, повтор, громкость, «что играет»,
«похожее/рекомендации» (волна по текущему исполнителю), генерик «включи музыку».

## Требования

- Яндекс-Станция с root + ADB и установленной [Luna](https://github.com/nyakokitsu/luna) через
  [YAM](https://github.com/nyakokitsu/yam) (проверено на Станции Мини 2, `yandexmini_2`).
- Home Assistant с интеграцией Music Assistant и `media_player` колонки, плюс долгоживущий токен.

## Сборка

Нужен Rust (rustup). Артефакты — в `dist/`.

```sh
sh scripts/build-arm.sh          # armv7 (Linaro, glibc ≤2.23) -> dist/soundya-armv7.tar

# Пакет Luna (.yampkg):
git clone https://github.com/nyakokitsu/yam ../yam
(cd ../yam && cargo build --release --bin yam-publish)
YAM_SRC=../yam YAM_PACK_ONLY=1 YAM_TARGETS=yandexmini_2 sh scripts/yam-release.sh
```

## Установка (пакетом Luna)

```sh
adb push dist/soundya-<версия>-yandexmini_2.yampkg /tmp/ma.yampkg
# Luna держит YAM-lock — на время установки её надо остановить:
ssh root@<колонка> '/etc/init.d/S99luna stop && \
  /data/yam/bin/yam-run install --yes /tmp/ma.yampkg && \
  /etc/init.d/S99luna start'
```

Два soundya одновременно нельзя (общий тап голоса, порт 8765, AirPlay) — ставить **вместо**
штатного soundya. Автономный путь из апстрима (`scripts/deploy.sh`) тоже работает — см.
[docs/README.upstream-soundya.md](docs/README.upstream-soundya.md).

## Настройка Music Assistant

Открой `http://<колонка>:8765/` → раздел **Music Assistant**:

- **Адрес Home Assistant** — напр. `http://homeassistant.local:8123`
- **Плеер колонки** — `media_player.*` колонки в MA/HA
- **Токен** — долгоживущий токен Home Assistant (хранится только на колонке, в UI маскируется)
- **Тип по умолчанию** и **Диджей (radio_mode)** — по вкусу

## Отличия от soundya

- `src/ma.rs` — **новый**: клиент REST Home Assistant.
- `src/alice.rs` — маршрутизация (музыка → MA по умолчанию, «саундклауд» → SoundCloud; транспорт
  за активным источником; `keyword_only` не применяется при включённом MA; генерик «включи
  музыку» → MA).
- `src/main.rs` — `Cmd::Voice(Intent, bool)`, `ma_intent`, `Shared.ma_enabled/ma_active`.
- `src/settings.rs` — `ma_enabled/ha_url/ha_token/ma_entity/ma_radio/ma_type`.
- `src/web.html` — карточка настроек Music Assistant.
- `src/{buttons,luna}.rs` — кнопки и команды Luna идут в активный источник.

Всё, что soundya умел раньше (SoundCloud голосом, AirPlay, ПК/ТВ, обход блокировок), сохранено.

## Благодарности и лицензия

MIT, как и апстрим. На основе **soundya** от [rodyatotu](https://github.com/rodyatotu/soundya),
который основан на **spotiya** от [klockky](https://github.com/klockky/spotiya). См.
[LICENSE](LICENSE) и [docs/README.upstream-soundya.md](docs/README.upstream-soundya.md).
