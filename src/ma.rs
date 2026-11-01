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

use std::time::Duration;

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
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
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

/// «Включи X»: поиск по имени и воспроизведение на колонке через MA.
/// `media_type` None → cfg.default_type. `radio` None → cfg.radio.
pub fn play(cfg: &Cfg, query: &str, media_type: Option<&str>, radio: Option<bool>) -> Res<()> {
    let mtype = media_type.unwrap_or(&cfg.default_type);
    let mut data = serde_json::json!({
        "media_id": query,
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
