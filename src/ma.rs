//! Music Assistant backend через REST API Home Assistant.
//!
//! Голосовой слой (alice.rs) распознаёт «Алиса, включи X» и отдаёт Intent.
//! Этот модуль НЕ играет звук сам: он зовёт сервисы Home Assistant, а MA
//! воспроизводит на плеере-колонке своим провайдером (путь glagol+mediad).
//!
//! Воспроизведение: `music_assistant.play_media` (поиск по имени + radio_mode
//! для «диджей»-бесконечного потока). Транспорт/громкость: `media_player.*`.
//! «Что играет»: `GET /api/states/<entity>`.
//!
//! Конфиг (из settings.rs, заполняет пользователь — токен в файлы не писать):
//!   ha_url    напр. http://192.168.3.8:8123
//!   ha_token  long-lived access token Home Assistant (из DJ-интеграции)
//!   ma_entity media_player.* колонки (напр. media_player.yandexioreceiver_...)
//!   ma_radio  включать radio_mode (диджей) для «включи <артист>»
//!   ma_type   media_type по умолчанию для «включи X» (artist|track|...)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::local;
use crate::settings::Settings;

#[derive(Debug, Clone)]
pub struct Cfg {
    pub url: String,
    pub token: String,
    pub entity: String,
    pub radio: bool,
    pub default_type: String,
}

impl Cfg {
    /// Берёт настройки MA из Settings; None, если мост не настроен.
    pub fn from_settings(s: &Settings) -> Option<Cfg> {
        let url = s.ha_url.trim().trim_end_matches('/').to_string();
        let token = s.ha_token.trim().to_string();
        let entity = s.ma_entity.trim().to_string();
        if url.is_empty() || token.is_empty() || entity.is_empty() {
            return None;
        }
        let default_type = if s.ma_type.trim().is_empty() {
            "artist".to_string()
        } else {
            s.ma_type.trim().to_string()
        };
        Some(Cfg {
            url,
            token,
            entity,
            radio: s.ma_radio,
            default_type,
        })
    }
}

pub type Res<T> = Result<T, String>;

fn agent() -> ureq::Agent {
    // Генерация диджей-волны (radio_mode) в MA может длиться десятки секунд —
    // HA держит HTTP-ответ до её конца, поэтому читаем ответ долго. Сам вызов
    // идёт в фоне (см. ma_intent), цикл команд этим не блокируется.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(35))
        .timeout_write(Duration::from_secs(10))
        .build()
}

/// POST /api/services/<domain>/<service> с телом data (entity_id внутри data —
/// REST API HA принимает target так же).
fn call_service(cfg: &Cfg, domain: &str, service: &str, mut data: serde_json::Value) -> Res<()> {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("entity_id".into(), serde_json::json!(cfg.entity));
    }
    let url = format!("{}/api/services/{}/{}", cfg.url, domain, service);
    match agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("Content-Type", "application/json")
        .send_json(data)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(format!("HA {domain}.{service}: HTTP {code} {}", body.trim()))
        }
        Err(e) => Err(format!("HA {domain}.{service}: {e}")),
    }
}

/// config_entry_id интеграции Music Assistant в HA (нужен сервису search).
/// Берём через шаблон по сущности плеера и кэшируем — он не меняется.
static CONFIG_ENTRY: OnceLock<String> = OnceLock::new();

fn config_entry_id(cfg: &Cfg) -> Option<String> {
    if let Some(v) = CONFIG_ENTRY.get() {
        return Some(v.clone());
    }
    let url = format!("{}/api/template", cfg.url);
    let body = serde_json::json!({
        "template": format!("{{{{ config_entry_id('{}') }}}}", cfg.entity)
    });
    let s = agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("Content-Type", "application/json")
        .send_json(body)
        .ok()?
        .into_string()
        .ok()?
        .trim()
        .to_string();
    if s.is_empty() || s == "None" {
        return None;
    }
    let _ = CONFIG_ENTRY.set(s.clone());
    Some(s)
}

fn plural(mtype: &str) -> &'static str {
    match mtype {
        "artist" => "artists",
        "album" => "albums",
        "playlist" => "playlists",
        "radio" => "radio",
        "podcast" => "podcasts",
        "audiobook" => "audiobooks",
        _ => "tracks",
    }
}

/// Один поиск через сервис `music_assistant.search`. Возвращает uri первого
/// подходящего результата нужного типа. `library_only` — только локальная
/// библиотека (локальные файлы). `want_prefix` — брать первый результат, чей uri
/// начинается с этого префикса (напр. "spotify--"), иначе первый вообще.
fn search_uri(
    cfg: &Cfg,
    ce: &str,
    query: &str,
    mtype: &str,
    library_only: bool,
    want_prefix: Option<&str>,
) -> Option<String> {
    let mut body = serde_json::json!({
        "config_entry_id": ce,
        "name": query,
        "media_type": [mtype],
        "limit": 8,
    });
    if library_only {
        body["library_only"] = serde_json::json!(true);
    }
    let url = format!(
        "{}/api/services/music_assistant/search?return_response=true",
        cfg.url
    );
    let v: serde_json::Value = agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("Content-Type", "application/json")
        .send_json(body)
        .ok()?
        .into_json()
        .ok()?;
    let items = v["service_response"][plural(mtype)].as_array()?;
    match want_prefix {
        Some(pfx) => items.iter().find_map(|it| {
            let u = it["uri"].as_str()?;
            u.starts_with(pfx).then(|| u.to_string())
        }),
        None => items
            .first()
            .and_then(|it| it["uri"].as_str().map(str::to_string)),
    }
}

/// «Включи X» ставит X в винительный падеж: «Агутина», «Земфиру», «Сплина».
/// Ни резолвер имён play_media, ни поиск склонённую форму не находят (search
/// «агутина» → 0, «агутин» → есть). Отбрасываем одну хвостовую гласную у последнего
/// слова, чтобы получить основу ближе к именительному. Короткие слова не трогаем
/// (их основа и так ищется). None — отбрасывать нечего.
fn ru_stem(query: &str) -> Option<String> {
    let q = query.trim();
    let (head, last) = q.rsplit_once(' ').map_or(("", q), |(h, l)| (h, l));
    let mut chars: Vec<char> = last.chars().collect();
    // хвостовые гласные вин./род. падежа: а я у ю е ё и й
    if chars.len() >= 5 && matches!(chars.last(), Some('а' | 'я' | 'у' | 'ю' | 'е' | 'ё' | 'и' | 'й')) {
        chars.pop();
        let stem: String = chars.into_iter().collect();
        return Some(if head.is_empty() { stem } else { format!("{head} {stem}") });
    }
    None
}

/// Правило пользователя: сначала локальная библиотека (локальные файлы), затем
/// обычный поиск (Spotify, т.к. Yandex Music отключён). Возвращает media_id для
/// play_media: найденный uri, иначе — имя как есть (пусть MA пробует сам).
fn resolve_media_id(cfg: &Cfg, query: &str, mtype: &str) -> String {
    let Some(ce) = config_entry_id(cfg) else {
        return query.to_string();
    };
    // 1. Локальная библиотека — быстро (~20 мс) и в приоритете.
    if let Some(uri) = search_uri(cfg, &ce, query, mtype, true, None) {
        eprintln!("ma: «{query}» → локально {uri}");
        return uri;
    }
    // 2. Нелокальный поиск. play_media по сырому имени падежи не понимает
    //    («включи Агутина» → «агутина» → MA «Could not resolve»), а search вернёт uri.
    if let Some(uri) = search_uri(cfg, &ce, query, mtype, false, None) {
        eprintln!("ma: «{query}» → {uri}");
        return uri;
    }
    // 3. Винительный падеж: пробуем основу без хвостовой гласной («агутина» → «агутин»).
    if let Some(stem) = ru_stem(query) {
        if let Some(uri) = search_uri(cfg, &ce, &stem, mtype, false, None) {
            eprintln!("ma: «{query}» → основа «{stem}» → {uri}");
            return uri;
        }
    }
    // 4. Ничего не нашли поиском — отдаём имя, пусть MA пробует сам.
    eprintln!("ma: «{query}» → не найдено поиском, отдаём MA как есть");
    query.to_string()
}

/// Запрос-жанр («джаз», «рок», «классика»…), а не исполнитель. Такое лучше играть
/// как плейлист (жанровые подборки), а не искать исполнителя с таким именем.
pub fn is_genre(query: &str) -> bool {
    let q = query.trim().to_lowercase().replace('ё', "е");
    const GENRES: [&str; 34] = [
        "джаз", "рок", "поп", "классик", "блюз", "метал", "рэп", "хип-хоп", "хипхоп", "хип хоп",
        "электрон", "техно", "хаус", "диско", "фанк", "соул", "регги", "регей", "кантри", "шансон",
        "инди", "панк", "лоуфай", "лоу-фай", "lofi", "эмбиент", "транс", "драм-н-бейс", "дабстеп",
        "опера", "рнб", "r&b", "релакс", "ретро",
    ];
    // Один-два слова, совпадающих со стержнем жанра («джаз», «классическую», «рок музыка»).
    let first = q.split_whitespace().next().unwrap_or(&q);
    GENRES.iter().any(|g| first.starts_with(g) || q == *g)
}

// ---------------------------------------------------------------------------
// Локальный поиск: индекс библиотеки MA в памяти колонки (см. local.rs).
// ---------------------------------------------------------------------------

/// Индекс и время его построения.
static LOCAL: Mutex<Option<(Instant, Arc<Vec<local::Item>>)>> = Mutex::new(None);
static LOCAL_BUILDING: AtomicBool = AtomicBool::new(false);
/// Индекс перестраивается в фоне, если старше этого.
const LOCAL_TTL: Duration = Duration::from_secs(15 * 60);
/// Не нашли, а индексу больше 5 минут — перестроить сразу и поискать ещё раз
/// (подхватит только что добавленную музыку).
const LOCAL_RETRY_AGE: Duration = Duration::from_secs(5 * 60);

/// Список библиотеки MA одного типа (`music_assistant.get_library`).
fn get_library(cfg: &Cfg, ce: &str, mtype: &str) -> Res<Vec<serde_json::Value>> {
    let url = format!("{}/api/services/music_assistant/get_library?return_response=true", cfg.url);
    let body = serde_json::json!({ "config_entry_id": ce, "media_type": mtype, "limit": 20000 });
    let v: serde_json::Value = agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| format!("get_library {mtype}: {e}"))?
        .into_json()
        .map_err(|e| format!("get_library {mtype}: {e}"))?;
    v["service_response"]["items"]
        .as_array()
        .cloned()
        .ok_or_else(|| format!("get_library {mtype}: нет items в ответе"))
}

fn build_local(cfg: &Cfg) -> Res<Arc<Vec<local::Item>>> {
    let t0 = Instant::now();
    let ce = config_entry_id(cfg).ok_or("нет config_entry_id Music Assistant")?;
    let artists = get_library(cfg, &ce, "artist")?;
    let albums = get_library(cfg, &ce, "album")?;
    let tracks = get_library(cfg, &ce, "track")?;
    let playlists = get_library(cfg, &ce, "playlist")?;
    let idx = Arc::new(local::build(&artists, &albums, &tracks, &playlists));
    let count = |k| idx.iter().filter(|i| i.kind == k).count();
    eprintln!(
        "ma: локальный индекс: артистов {}, альбомов {}, треков {}, плейлистов {} ({} мс)",
        count(local::Kind::Artist),
        count(local::Kind::Album),
        count(local::Kind::Track),
        count(local::Kind::Playlist),
        t0.elapsed().as_millis()
    );
    *LOCAL.lock().unwrap() = Some((Instant::now(), idx.clone()));
    Ok(idx)
}

/// Перестроить индекс в фоне (на старте и по истечении LOCAL_TTL).
pub fn refresh_local(cfg: Cfg) {
    if LOCAL_BUILDING.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        if let Err(e) = build_local(&cfg) {
            eprintln!("ma: локальный индекс не построен: {e}");
        }
        LOCAL_BUILDING.store(false, Ordering::Release);
    });
}

/// Текущий индекс и его возраст; если индекса нет — строит сразу (~3 с).
fn local_index(cfg: &Cfg) -> Res<(Arc<Vec<local::Item>>, Duration)> {
    let cur = LOCAL.lock().unwrap().clone();
    match cur {
        Some((at, idx)) => {
            if at.elapsed() > LOCAL_TTL {
                refresh_local(cfg.clone());
            }
            Ok((idx, at.elapsed()))
        }
        None => build_local(cfg).map(|idx| (idx, Duration::ZERO)),
    }
}

/// Команда `find X`: что нашёл бы локальный поиск — без воспроизведения.
pub fn find_local_text(cfg: &Cfg, query: &str) -> String {
    match local_index(cfg) {
        Ok((idx, age)) => match local::find(&idx, query, None) {
            Some(h) => format!(
                "OK {} «{}» {} [{:.1}] (индекс {} шт., возраст {} с)",
                h.item.kind.media_type(),
                h.item.name,
                h.item.uri,
                h.score,
                idx.len(),
                age.as_secs()
            ),
            None => {
                let sim = local::find_similar(&idx, query, SIMILAR_MAX);
                if sim.is_empty() {
                    format!("NOT FOUND «{query}» (индекс {} шт., возраст {} с)", idx.len(), age.as_secs())
                } else {
                    let list: Vec<String> = sim
                        .iter()
                        .take(8)
                        .map(|i| format!("«{}»", i.name))
                        .collect();
                    format!("SIMILAR {} шт.: {}{}", sim.len(), list.join(", "), if sim.len() > 8 { " …" } else { "" })
                }
            }
        },
        Err(e) => format!("ERR {e}"),
    }
}

/// Сколько песен ставить в очередь «похожих».
const SIMILAR_MAX: usize = 30;

/// Чем закончился локальный «включи X».
pub enum Local {
    /// Точное совпадение — играет оно.
    Exact(String),
    /// Точного нет, есть N похожих — НЕ играют, ждут ответа (см. Pending).
    Similar(usize),
    /// Ничего общего — ничего не запущено.
    NotFound,
}

/// Открытый вопрос «Включить похожие или поискать на Спотифай?».
struct Pending {
    at: Instant,
    query: String,
    uris: Vec<String>,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
/// Сколько ждём ответа на вопрос: отсчёт с момента, когда колонка начала слушать
/// (`pending_listening`), плюс запас на распознавание. До этого — пока Алиса задаёт вопрос.
const PENDING_TTL: Duration = Duration::from_secs(14);

/// Колонка начала слушать ответ — отсчёт окна ответа заново.
pub fn pending_listening() {
    if let Some(p) = PENDING.lock().unwrap().as_mut() {
        p.at = Instant::now();
    }
}

/// Открыт ли вопрос о похожих (ответ «да / на Спотифай / нет» сейчас наш).
pub fn pending_active() -> bool {
    PENDING.lock().unwrap().as_ref().is_some_and(|p| p.at.elapsed() < PENDING_TTL)
}

/// Ответ на вопрос о похожих: включить их, искать на Spotify или ничего.
pub fn answer_similar(cfg: &Cfg, a: crate::alice::Answer) -> Res<()> {
    use crate::alice::{say, Answer};
    let Some(p) = PENDING.lock().unwrap().take().filter(|p| p.at.elapsed() < PENDING_TTL) else {
        return Ok(());
    };
    match a {
        Answer::Yes => {
            eprintln!("ma: «{}» → включаю похожие ({})", p.query, p.uris.len());
            call_service(
                cfg,
                "music_assistant",
                "play_media",
                serde_json::json!({ "media_id": p.uris, "media_type": "track", "enqueue": "replace", "radio_mode": false }),
            )?;
            say("Это песни, похожие на то, что вы запрашивали".into());
        }
        Answer::Spotify => {
            if !play_spotify(cfg, &p.query)? {
                say(format!("На Спотифай тоже не нашла: {}", p.query));
            }
        }
        // «Нет» — пауза и тишина: ни Алисы (срезана), ни наших слов (решение 02.11).
        Answer::No => {
            eprintln!("ma: «{}» → нет: пауза", p.query);
            pause(cfg)?;
        }
    }
    Ok(())
}

/// Поиск на Spotify (через поиск MA, только uri Spotify): артист, трек, альбом;
/// запрос как есть, без слова-подсказки и основой без падежного окончания.
/// true — нашли и запустили.
fn play_spotify(cfg: &Cfg, query: &str) -> Res<bool> {
    let ce = config_entry_id(cfg).ok_or("нет config_entry_id Music Assistant")?;
    let bare = crate::alice::music_query(query).to_owned();
    // Spotify сам падежи не понимает («агутина» → пусто): ищем и основой без окончания.
    let mut variants = vec![bare.clone()];
    if let Some(stem) = ru_stem(&bare) {
        variants.push(stem);
    }
    // Кандидаты всех типов, а выбор — тем же строгим сопоставлением, что в библиотеке:
    // иначе первый артист выдачи («хочу перемен» → Peremotka) побеждает песню.
    let mut items = Vec::new();
    for q in &variants {
        let url = format!("{}/api/services/music_assistant/search?return_response=true", cfg.url);
        let body = serde_json::json!({
            "config_entry_id": ce, "name": q, "media_type": ["artist", "album", "track"], "limit": 10,
        });
        let resp: serde_json::Value = match agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {}", cfg.token))
            .set("Content-Type", "application/json")
            .send_json(body)
        {
            Ok(r) => r.into_json().unwrap_or_default(),
            Err(e) => return Err(format!("поиск Spotify «{q}»: {e}")),
        };
        items.extend(local::from_search(&resp["service_response"], "spotify"));
    }
    let Some(hit) = local::find_spotify(&items, query) else {
        let seen: Vec<String> = items.iter().take(6).map(|i| format!("{} «{}»", i.kind.media_type(), i.name)).collect();
        eprintln!("ma: «{query}» → на Spotify подходящего нет (кандидаты: {seen:?})");
        return Ok(false);
    };
    let (mtype, uri, name) = (hit.item.kind.media_type(), hit.item.uri.clone(), hit.item.name.clone());
    eprintln!("ma: «{query}» → Spotify {mtype} «{name}» {uri} [{:.1}]", hit.score);
    call_service(
        cfg,
        "music_assistant",
        "play_media",
        serde_json::json!({ "media_id": uri, "media_type": mtype, "radio_mode": false }),
    )?;
    Ok(true)
}

/// «Включи X» — ТОЛЬКО локальная библиотека: точное совпадение, иначе похожие
/// (не включаются сразу — открывается вопрос, см. answer_similar), иначе ничего.
pub fn play_local(cfg: &Cfg, query: &str, prefer: Option<local::Kind>) -> Res<Local> {
    let (mut idx, age) = local_index(cfg)?;
    let mut hit = local::find(&idx, query, prefer).map(|h| (h.item.clone(), h.score));
    if hit.is_none() && age > LOCAL_RETRY_AGE {
        idx = build_local(cfg)?;
        hit = local::find(&idx, query, prefer).map(|h| (h.item.clone(), h.score));
    }
    if let Some((item, score)) = hit {
        let mtype = item.kind.media_type();
        eprintln!("ma: «{query}» → локально {mtype} «{}» {} [{score:.1}]", item.name, item.uri);
        call_service(
            cfg,
            "music_assistant",
            "play_media",
            serde_json::json!({ "media_id": item.uri, "media_type": mtype, "radio_mode": false }),
        )?;
        return Ok(Local::Exact(item.name));
    }
    let similar = local::find_similar(&idx, query, SIMILAR_MAX);
    if similar.is_empty() {
        eprintln!("ma: «{query}» → локально не найдено");
        return Ok(Local::NotFound);
    }
    let names: Vec<&str> = similar.iter().take(5).map(|i| i.name.as_str()).collect();
    eprintln!("ma: «{query}» → точно нет, похожих {}: {names:?}… — спрашиваю", similar.len());
    *PENDING.lock().unwrap() = Some(Pending {
        at: Instant::now(),
        query: query.to_owned(),
        uris: similar.iter().map(|i| i.uri.clone()).collect(),
    });
    Ok(Local::Similar(similar.len()))
}

/// «Включи X»: по правилу провайдеров (локальное → Spotify) находит конкретный
/// элемент и играет его на колонке через MA.
/// `media_type` None → cfg.default_type. `radio` None → cfg.radio.
pub fn play(cfg: &Cfg, query: &str, media_type: Option<&str>, radio: Option<bool>) -> Res<()> {
    let mtype = media_type.unwrap_or(&cfg.default_type);
    let media_id = resolve_media_id(cfg, query, mtype);
    let mut data = serde_json::json!({
        "media_id": media_id,
        "radio_mode": radio.unwrap_or(cfg.radio),
    });
    if !mtype.is_empty() {
        data["media_type"] = serde_json::json!(mtype);
    }
    call_service(cfg, "music_assistant", "play_media", data)
}

pub fn pause(cfg: &Cfg) -> Res<()> {
    call_service(cfg, "media_player", "media_pause", serde_json::json!({}))
}

pub fn resume(cfg: &Cfg) -> Res<()> {
    call_service(cfg, "media_player", "media_play", serde_json::json!({}))
}

pub fn next(cfg: &Cfg) -> Res<()> {
    call_service(cfg, "media_player", "media_next_track", serde_json::json!({}))
}

pub fn previous(cfg: &Cfg) -> Res<()> {
    call_service(cfg, "media_player", "media_previous_track", serde_json::json!({}))
}

/// Перемотка на `by_ms` относительно текущей позиции (может быть отрицательной).
pub fn seek(cfg: &Cfg, by_ms: i64) -> Res<()> {
    let now = now_playing(cfg)?;
    let pos = now.position_s.unwrap_or(0.0);
    let target = (pos + by_ms as f64 / 1000.0).max(0.0);
    call_service(
        cfg,
        "media_player",
        "media_seek",
        serde_json::json!({ "seek_position": target }),
    )
}

pub fn set_shuffle(cfg: &Cfg, on: bool) -> Res<()> {
    call_service(
        cfg,
        "media_player",
        "shuffle_set",
        serde_json::json!({ "shuffle": on }),
    )
}

/// repeat: true → повтор одного трека («one»), false → выкл.
pub fn set_repeat(cfg: &Cfg, on: bool) -> Res<()> {
    call_service(
        cfg,
        "media_player",
        "repeat_set",
        serde_json::json!({ "repeat": if on { "one" } else { "off" } }),
    )
}

pub fn set_volume(cfg: &Cfg, level_0_100: u8) -> Res<()> {
    let lvl = (level_0_100.min(100) as f64) / 100.0;
    call_service(
        cfg,
        "media_player",
        "volume_set",
        serde_json::json!({ "volume_level": lvl }),
    )
}

#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    pub state: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub position_s: Option<f64>,
    pub playing: bool,
}

/// GET /api/states/<entity> — состояние плеера колонки в MA/HA.
pub fn now_playing(cfg: &Cfg) -> Res<NowPlaying> {
    let url = format!("{}/api/states/{}", cfg.url, cfg.entity);
    let resp = match agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return Err(format!("HA state: HTTP {code} {}", body.trim()));
        }
        Err(e) => return Err(format!("HA state: {e}")),
    };
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("HA state parse: {e}"))?;
    let state = v["state"].as_str().unwrap_or("").to_string();
    let attr = &v["attributes"];
    let str_attr = |k: &str| attr[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Ok(NowPlaying {
        playing: state == "playing",
        title: str_attr("media_title"),
        artist: str_attr("media_artist"),
        position_s: attr["media_position"].as_f64(),
        state,
    })
}

/// Фраза для ответа Алисе «что играет».
pub fn what_playing_phrase(cfg: &Cfg) -> String {
    match now_playing(cfg) {
        Ok(np) if np.title.is_some() => match (&np.title, &np.artist) {
            (Some(t), Some(a)) => format!("{a} — {t}"),
            (Some(t), None) => t.clone(),
            _ => "Сейчас ничего не играет".into(),
        },
        Ok(_) => "Сейчас ничего не играет".into(),
        Err(_) => "Не удалось узнать, что играет".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ru_stem_drops_accusative_vowel() {
        assert_eq!(ru_stem("агутина").as_deref(), Some("агутин"));
        assert_eq!(ru_stem("земфиру").as_deref(), Some("земфир"));
        assert_eq!(ru_stem("леонида агутина").as_deref(), Some("леонида агутин"));
        // короткое слово и согласная на конце — не трогаем
        assert_eq!(ru_stem("кино"), None);        // < 5 букв
        assert_eq!(ru_stem("сплин"), None);       // согласная на конце
        assert_eq!(ru_stem("ддт"), None);
    }
}
