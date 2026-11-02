//! Поиск по локальной библиотеке MA: сопоставление голосового запроса с
//! артистами, альбомами, треками и плейлистами без обращения к поиску MA.
//!
//! Почему не поиск MA: он ищет подстроку, а голосовой запрос приходит в падеже
//! («включи Агутина», «белую ворону»), кириллицей для латинских имён («битлз»,
//! «элтона джона») и с лишними словами («группу кино»). Поэтому колонка держит
//! список библиотеки (ma.rs) и сопоставляет сама:
//!   3   — слово совпало точно (после транслита в латиницу);
//!   2   — та же основа, отличаются только падежные окончания («агаты» ↔ «агата»);
//!   1.5 — кириллица ↔ латиница звучат одинаково по согласным («квин» ↔ «queen»);
//!   1.2 — то же, но по двум согласным («джона» ↔ «john»), только в составе фразы;
//!   1   — по согласным, одна ошибка, от 4 согласных.
//! Отлажено на реальной библиотеке (98 артистов, 225 альбомов, 2381 трек), см. LOG.md 02.11.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Artist,
    Album,
    Track,
    Playlist,
}

impl Kind {
    pub fn media_type(self) -> &'static str {
        match self {
            Kind::Artist => "artist",
            Kind::Album => "album",
            Kind::Track => "track",
            Kind::Playlist => "playlist",
        }
    }
    fn bonus(self) -> f32 {
        match self {
            Kind::Artist => 3.0,
            Kind::Album => 2.0,
            Kind::Playlist => 1.5,
            Kind::Track => 1.0,
        }
    }
}

/// Слово названия с заранее посчитанными формами (считаются при построении индекса).
#[derive(Debug, Clone)]
struct Tok {
    raw: String,
    lat: String,
    skel: String,
    cyr: bool,
    stop: bool,
}

impl Tok {
    fn new(raw: String) -> Tok {
        Tok { lat: lat(&raw), skel: skel(&raw), cyr: is_cyr(&raw), stop: STOP.contains(&raw.as_str()), raw }
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub uri: String,
    /// Слова названия.
    name_toks: Vec<Tok>,
    /// Слова исполнителей (у альбомов и треков).
    extra_toks: Vec<Tok>,
    /// Значимые слова названия — без скобок, цифр и предлогов: по ним считаем покрытие.
    core_toks: Vec<Tok>,
}

#[derive(Debug, Clone)]
pub struct Hit<'a> {
    pub item: &'a Item,
    pub score: f32,
}

/// Служебные слова, которые голосом ставят перед названием.
const FILLER: [&str; 17] = [
    "группу", "группа", "группы", "песню", "песня", "песни", "трек", "альбом", "исполнителя",
    "исполнитель", "певца", "певицу", "музыку", "музыка", "пожалуйста", "мне", "нам",
];
/// Предлоги и артикли: совпадают только точно и не считаются в покрытии.
const STOP: [&str; 21] = [
    "the", "a", "an", "of", "and", "co", "feat", "в", "на", "по", "и", "с", "у", "к", "о", "об",
    "из", "за", "от", "до", "не",
];
/// Частые слова: для «похожих» не считаются — совпадают почти с любым названием.
const COMMON: [&str; 40] = [
    "про", "под", "над", "для", "без", "при", "что", "как", "это", "мой", "моя", "мое", "мои",
    "твой", "твоя", "твои", "я", "ты", "он", "она", "мы", "вы", "они", "все", "так", "там", "тут",
    "вот", "же", "бы", "ли", "нет", "да", "меня", "тебя", "мне", "тебе", "нас", "вас", "когда",
];
/// Падежные окончания в латинице.
const ENDINGS: [&str; 39] = [
    "", "a", "ya", "u", "yu", "e", "i", "y", "o", "om", "em", "oi", "ei", "ov", "ev", "ami", "yami",
    "ah", "yah", "ogo", "ego", "omu", "emu", "uyu", "yuyu", "aya", "yaya", "oe", "ee", "ye", "ie",
    "yi", "ii", "ym", "im", "yh", "ih", "ia", "iu",
];
/// Плейлисты, которые не локальные: служебные MA, Spotify, остатки Яндекса.
const SKIP_PLAYLISTS: [&str; 12] = [
    "сектор газа лучшее", "liked songs", "микс дня", "my wave", "мой плейлист", "наш плейлист",
    "infinite mix", "random", "recently", "all favorited", "my favorites", "from library",
];

pub fn norm(s: &str) -> Vec<String> {
    let s: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'ё' => 'е',
            // украинские и белорусские буквы — к ближайшим русским, иначе слово рвётся
            // («Ікони» → «кони»)
            'і' | 'ї' => 'и',
            'є' => 'е',
            'ґ' => 'г',
            'ў' => 'у',
            'a'..='z' | '0'..='9' | 'а'..='я' => c,
            _ => ' ',
        })
        .collect();
    s.split_whitespace().map(str::to_owned).collect()
}

fn core_name(s: &str) -> Vec<Tok> {
    // Без содержимого скобок: «(Диск 1)», «[01]», «(акустика)».
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
        if matches!(c, '(' | '[' | ')' | ']') {
            out.push(' ');
        }
    }
    norm(&out)
        .into_iter()
        .filter(|w| {
            !w.chars().all(|c| c.is_ascii_digit())
                // инициалы: «Песни Б. Окуджавы», «F.R. David»
                && w.chars().count() > 1
                && !STOP.contains(&w.as_str())
                // «Песни Вертинского», «Синий альбом» — содержательная часть без этих слов
                && !FILLER.contains(&w.as_str())
                && !matches!(w.as_str(), "диск" | "disc" | "cd" | "бонус" | "bonus" | "песенки" | "songs" | "album")
        })
        .map(Tok::new)
        .collect()
}

pub fn lat(w: &str) -> String {
    let mut o = String::with_capacity(w.len());
    for c in w.chars() {
        o.push_str(match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' | 'э' => "e",
            'ж' => "zh", 'з' => "z", 'и' | 'й' => "i", 'к' => "k", 'л' => "l", 'м' => "m",
            'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u",
            'ф' => "f", 'х' => "h", 'ц' => "c", 'ч' => "ch", 'ш' => "sh", 'щ' => "sch",
            'ъ' | 'ь' => "", 'ы' => "y", 'ю' => "yu", 'я' => "ya",
            _ => {
                o.push(c);
                continue;
            }
        });
    }
    o
}

fn is_cyr(s: &str) -> bool {
    s.chars().any(|c| ('а'..='я').contains(&c))
}

/// Скелет по согласным для сравнения кириллицы с латиницей.
pub fn skel(w: &str) -> String {
    // «уайлд» → w: «у» перед гласной в начале слова или после гласной.
    let cs: Vec<char> = w.chars().collect();
    let vowel = |c: char| "аеиоуыэюя".contains(c);
    let mut fixed = String::new();
    for (i, &c) in cs.iter().enumerate() {
        let prev_ok = i == 0 || vowel(cs[i - 1]);
        let next_ok = cs.get(i + 1).is_some_and(|&n| "аеиоэ".contains(n));
        fixed.push(if c == 'у' && prev_ok && next_ok { 'в' } else { c });
    }
    let mut s = lat(&fixed);
    for (a, b) in [
        ("sch", "S"), ("chael", "kel"), ("chr", "kr"), ("ch", "S"), ("sh", "S"), ("ph", "f"),
        ("th", "t"), ("ck", "k"), ("qu", "kv"), ("x", "ks"), ("w", "v"), ("dzh", "j"), ("zh", "j"),
        ("z", "s"),
    ] {
        s = s.replace(a, b);
    }
    let b: Vec<char> = s.chars().collect();
    let mut t = String::new();
    for (i, &c) in b.iter().enumerate() {
        match c {
            'c' if b.get(i + 1).is_some_and(|n| "eiy".contains(*n)) => t.push('s'),
            'c' => t.push('k'),
            'h' | 'a' | 'e' | 'i' | 'o' | 'u' | 'y' => {}
            _ => t.push(c),
        }
    }
    let mut out = String::new();
    for c in t.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn lev(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut p: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut c = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            c.push((p[j + 1] + 1).min(c[j] + 1).min(p[j] + usize::from(ca != cb)));
        }
        p = c;
    }
    p[b.len()]
}

/// Насколько слово запроса `q` совпадает со словом названия `t` (см. шкалу в шапке).
pub fn tok_level(q: &str, t: &str) -> f32 {
    level(&Tok::new(q.to_owned()), &Tok::new(t.to_owned()))
}

fn level(q: &Tok, t: &Tok) -> f32 {
    let (ql, tl) = (&q.lat, &t.lat);
    if ql == tl {
        return 3.0;
    }
    if t.stop || q.stop {
        return 0.0;
    }
    let qb = ql.as_bytes();
    let tb = tl.as_bytes();
    let mut k = 0;
    while k < qb.len().min(tb.len()) && qb[k] == tb[k] {
        k += 1;
    }
    for j in (2..=k).rev() {
        // латиница после транслита — ASCII, срез по байтам безопасен
        if ENDINGS.contains(&&ql[j..]) && ENDINGS.contains(&&tl[j..]) {
            return 2.0;
        }
    }
    if q.cyr == t.cyr {
        return 0.0;
    }
    let (qs, ts) = (&q.skel, &t.skel);
    if qs.len() >= 2 && qs == ts {
        return if qs.chars().count() >= 3 { 1.5 } else { 1.2 };
    }
    if qs.chars().count().min(ts.chars().count()) >= 4 && lev(qs, ts) == 1 {
        return 1.0;
    }
    0.0
}

fn hint(word: &str) -> Option<Kind> {
    match word {
        "песню" | "песня" | "трек" => Some(Kind::Track),
        "альбом" | "пластинку" => Some(Kind::Album),
        "плейлист" => Some(Kind::Playlist),
        _ => None,
    }
}

/// Лучшее совпадение в библиотеке или None («локально не найдено»).
/// `prefer` — какой тип предпочесть («включи альбом X»).
pub fn find<'a>(items: &'a [Item], query: &str, prefer: Option<Kind>) -> Option<Hit<'a>> {
    find_with(items, query, prefer, Kind::bonus)
}

/// Для выдачи Spotify: артист > песня > альбом (в библиотеке альбом выше песни —
/// «группа крови» → твой альбом; на Spotify одноимённый альбом обычно чей-то кавер).
/// При равенстве побеждает тот, кто выше в выдаче (порядок `items` сохраняется).
pub fn find_spotify<'a>(items: &'a [Item], query: &str) -> Option<Hit<'a>> {
    find_with(items, query, None, |k| match k {
        Kind::Artist => 3.0,
        Kind::Track => 2.0,
        Kind::Album => 1.0,
        Kind::Playlist => 0.5,
    })
}

fn find_with<'a>(items: &'a [Item], query: &str, prefer: Option<Kind>, bonus: fn(Kind) -> f32) -> Option<Hit<'a>> {
    let words: Vec<Tok> = norm(query).into_iter().map(Tok::new).collect();
    let mut variants = vec![(words.clone(), prefer)];
    if words.len() > 1 && (FILLER.contains(&words[0].raw.as_str()) || hint(&words[0].raw).is_some()) {
        variants.push((words[1..].to_vec(), prefer.or_else(|| hint(&words[0].raw))));
    }
    variants
        .iter()
        .filter_map(|(q, pr)| find_q(items, q, *pr, bonus))
        .max_by(|a, b| a.score.total_cmp(&b.score))
}

/// Точного совпадения нет — песни, похожие на запрос: треки, в НАЗВАНИИ которых
/// есть хотя бы одно значимое слово запроса — точно или в другом падеже.
/// Исполнитель и «звучание» не учитываются (решение пользователя 02.11: иначе
/// «кино про любовь» тянет всю группу Кино, «хочу» ~ «Hoch», «Белая гвардия»).
/// Больше совпавших слов — выше в списке; одна и та же песня с разных альбомов — один раз.
pub fn find_similar<'a>(items: &'a [Item], query: &str, limit: usize) -> Vec<&'a Item> {
    let words: Vec<Tok> = norm(query)
        .into_iter()
        .filter(|w| {
            !FILLER.contains(&w.as_str())
                && !STOP.contains(&w.as_str())
                && !COMMON.contains(&w.as_str())
                && hint(w).is_none()
        })
        .map(Tok::new)
        .filter(|t| t.lat.len() > 2)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f32, &Item)> = items
        .iter()
        .filter(|it| it.kind == Kind::Track)
        .filter_map(|it| {
            let (mut hits, mut sum) = (0usize, 0.0f32);
            for w in &words {
                // только тот же алфавит: иначе «хочу» ~ «Hoch», «кино» ~ «Kino»
                let lv = it.name_toks.iter().filter(|x| x.cyr == w.cyr).map(|x| level(w, x)).fold(0.0, f32::max);
                if lv >= 2.0 {
                    hits += 1;
                    sum += lv;
                }
            }
            (hits > 0).then_some((hits as f32 * 10.0 + sum, it))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .map(|(_, it)| it)
        .filter(|it| {
            let first_artist = it.extra_toks.first().map(|t| t.raw.as_str()).unwrap_or("");
            seen.insert(format!("{}|{first_artist}", it.name.to_lowercase()))
        })
        .take(limit)
        .collect()
}

fn find_q<'a>(items: &'a [Item], q: &[Tok], prefer: Option<Kind>, bonus: fn(Kind) -> f32) -> Option<Hit<'a>> {
    if q.is_empty() {
        return None;
    }
    let mut best: Option<Hit> = None;
    'items: for it in items {
        let toks: Vec<&Tok> = it.name_toks.iter().chain(&it.extra_toks).collect();
        if toks.is_empty() {
            continue;
        }
        let mut levels = Vec::with_capacity(q.len());
        for w in q {
            let mut lv = toks.iter().map(|x| level(w, x)).fold(0.0, f32::max);
            // инициал в названии: «булата» ↔ «Б.», «фр» ↔ «F.R.» — по первой букве
            if lv == 0.0 && w.lat.len() >= 2 {
                let initial = toks.iter().any(|x| !x.stop && x.raw.chars().count() == 1 && w.lat.starts_with(&x.lat));
                if initial {
                    lv = 2.0;
                }
            }
            // длинное слово запроса не нашлось — не то
            if lv == 0.0 && w.lat.len() > 3 {
                continue 'items;
            }
            // одно слово — только точно, падеж или звук от 3 согласных
            if q.len() == 1 && lv < 1.5 {
                continue 'items;
            }
            levels.push(lv);
        }
        if levels.iter().cloned().fold(0.0, f32::max) < 1.5 {
            continue;
        }
        let strong = levels.iter().filter(|&&l| l >= 1.2).count();
        if levels.contains(&0.0) && strong < 2 {
            continue;
        }
        let core: &[Tok] = if it.core_toks.is_empty() { &it.name_toks } else { &it.core_toks };
        let matched = core.iter().filter(|x| q.iter().any(|w| level(w, x) > 0.0)).count();
        let cov = if core.is_empty() { 0.0 } else { matched as f32 / core.len() as f32 };
        if q.len() == 1 && levels[0] < 2.0 && cov < 1.0 {
            continue;
        }
        if it.kind != Kind::Artist && cov < 0.6 {
            continue;
        }
        let score = levels.iter().sum::<f32>() / (3.0 * q.len() as f32) * 10.0
            + cov * 5.0
            + bonus(it.kind)
            + if Some(it.kind) == prefer { 6.0 } else { 0.0 };
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Hit { item: it, score });
        }
    }
    best
}

/// Кандидаты из ответа `music_assistant.search` (artists/albums/tracks/playlists):
/// только те, чей uri начинается с `prefix` (напр. "spotify"). Для поиска на Spotify
/// теми же правилами сопоставления, что и в библиотеке.
pub fn from_search(resp: &Value, prefix: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for (key, kind) in [("artists", Kind::Artist), ("albums", Kind::Album), ("tracks", Kind::Track), ("playlists", Kind::Playlist)] {
        for v in resp[key].as_array().into_iter().flatten() {
            if v["uri"].as_str().is_some_and(|u| u.starts_with(prefix)) {
                out.extend(item(kind, v));
            }
        }
    }
    out
}

fn item(kind: Kind, v: &Value) -> Option<Item> {
    let name = v["name"].as_str()?.to_owned();
    let uri = v["uri"].as_str()?.to_owned();
    let extra_toks = v["artists"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).flat_map(norm).map(Tok::new).collect())
        .unwrap_or_default();
    let name_toks = norm(&name).into_iter().map(Tok::new).collect();
    Some(Item { kind, name_toks, core_toks: core_name(&name), name, uri, extra_toks })
}

/// Индекс локального из ответов get_library. Локальным считаем то, у чего есть
/// треки: артисты без треков — хвосты импорта Яндекса/Spotify, они ведут в
/// отключённый провайдер. Плейлисты — без служебных и чужих.
pub fn build(artists: &[Value], albums: &[Value], tracks: &[Value], playlists: &[Value]) -> Vec<Item> {
    use std::collections::HashSet;
    let t_artists: HashSet<&str> = tracks
        .iter()
        .filter_map(|t| t["artists"].as_array())
        .flatten()
        .filter_map(|a| a["name"].as_str())
        .collect();
    let t_albums: HashSet<&str> = tracks.iter().filter_map(|t| t["album"]["uri"].as_str()).collect();
    let mut out = Vec::new();
    out.extend(
        artists
            .iter()
            .filter(|a| a["name"].as_str().is_some_and(|n| t_artists.contains(n)))
            .filter_map(|a| item(Kind::Artist, a)),
    );
    out.extend(
        albums
            .iter()
            .filter(|a| a["uri"].as_str().is_some_and(|u| t_albums.contains(u)))
            .filter_map(|a| item(Kind::Album, a)),
    );
    out.extend(tracks.iter().filter_map(|t| item(Kind::Track, t)));
    out.extend(
        playlists
            .iter()
            .filter(|p| {
                p["name"].as_str().is_some_and(|n| {
                    let n = n.to_lowercase();
                    !SKIP_PLAYLISTS.iter().any(|s| n.contains(s))
                })
            })
            .filter_map(|p| item(Kind::Playlist, p)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lib() -> Vec<Item> {
        let artists = [
            json!({"name": "Кино", "uri": "library://artist/1"}),
            json!({"name": "Аквариум", "uri": "library://artist/2"}),
            json!({"name": "Борис Гребенщиков", "uri": "library://artist/3"}),
            json!({"name": "Elton John", "uri": "library://artist/4"}),
            json!({"name": "The Beatles", "uri": "library://artist/5"}),
            json!({"name": "Ace Of Base", "uri": "library://artist/6"}),
            json!({"name": "Madonna", "uri": "library://artist/7"}), // без треков — не локальный
            json!({"name": "Queen", "uri": "library://artist/8"}),
            json!({"name": "Чайф", "uri": "library://artist/9"}),
        ];
        let albums = [
            json!({"name": "Группа крови", "uri": "library://album/1", "artists": [{"name": "Кино"}]}),
            json!({"name": "Синий альбом", "uri": "library://album/2", "artists": [{"name": "Аквариум"}]}),
            json!({"name": "Исполнение разрешено (Диск 1)", "uri": "library://album/3", "artists": [{"name": "Аквариум"}]}),
            json!({"name": "Египтянин", "uri": "library://album/4", "artists": [{"name": "Аквариум"}]}),
            json!({"name": "Песни Б. Окуджавы", "uri": "library://album/5", "artists": [{"name": "Аквариум"}]}),
        ];
        let tr = |n: &str, a: &str, al: &str| json!({"name": n, "uri": format!("library://track/{n}"), "artists": [{"name": a}], "album": {"uri": al}});
        let tracks = [
            tr("Группа крови", "Кино", "library://album/1"),
            tr("Кошка", "Кино", "library://album/1"),
            tr("Перемена Мест", "Аквариум", "library://album/2"),
            tr("Белая ворона", "Чайф", "library://album/2"),
            tr("Sun", "Аквариум", "library://album/3"),
            tr("Lady Madoona", "The Beatles", "library://album/4"),
            tr("Песня", "Борис Гребенщиков", "library://album/4"),
            tr("Your Song", "Elton John", "library://album/4"),
            tr("Happy Nation", "Ace Of Base", "library://album/4"),
            tr("Bohemian Rhapsody", "Queen", "library://album/4"),
            tr("Песенка о голубом шарике", "Аквариум", "library://album/5"),
        ];
        let playlists = [
            json!({"name": "гавайская музыка", "uri": "library://playlist/1"}),
            json!({"name": "Liked Songs Ostaninkf", "uri": "library://playlist/2"}),
        ];
        build(&artists, &albums, &tracks, &playlists)
    }

    fn f(items: &[Item], q: &str) -> Option<String> {
        find(items, q, None).map(|h| format!("{}:{}", h.item.kind.media_type(), h.item.name))
    }

    #[test]
    fn artists_in_any_case_and_script() {
        let l = lib();
        assert_eq!(f(&l, "кино").as_deref(), Some("artist:Кино"));
        assert_eq!(f(&l, "группу кино").as_deref(), Some("artist:Кино"));
        assert_eq!(f(&l, "аквариума").as_deref(), Some("artist:Аквариум"));
        assert_eq!(f(&l, "гребенщикова").as_deref(), Some("artist:Борис Гребенщиков"));
        assert_eq!(f(&l, "элтона джона").as_deref(), Some("artist:Elton John"));
        assert_eq!(f(&l, "битлз").as_deref(), Some("artist:The Beatles"));
        assert_eq!(f(&l, "эйс оф бэйс").as_deref(), Some("artist:Ace Of Base"));
        assert_eq!(f(&l, "квин").as_deref(), Some("artist:Queen"));
    }

    #[test]
    fn songs_and_albums() {
        let l = lib();
        assert_eq!(f(&l, "белую ворону").as_deref(), Some("track:Белая ворона"));
        assert_eq!(f(&l, "чайф белая ворона").as_deref(), Some("track:Белая ворона"));
        assert_eq!(f(&l, "группу крови").as_deref(), Some("album:Группа крови"));
        assert_eq!(f(&l, "песню группа крови").as_deref(), Some("track:Группа крови"));
        assert_eq!(f(&l, "синий альбом").as_deref(), Some("album:Синий альбом"));
        // инициалы в названии не мешают
        assert_eq!(f(&l, "окуджаву").as_deref(), Some("album:Песни Б. Окуджавы"));
        assert_eq!(f(&l, "песни окуджавы").as_deref(), Some("album:Песни Б. Окуджавы"));
        assert_eq!(f(&l, "булата окуджаву").as_deref(), Some("album:Песни Б. Окуджавы"));
        // «альбом/песни» в названии не мешают: «синий» → «Синий альбом»
        assert_eq!(f(&l, "синий").as_deref(), Some("album:Синий альбом"));
        assert_eq!(f(&l, "исполнение разрешено").as_deref(), Some("album:Исполнение разрешено (Диск 1)"));
        assert_eq!(f(&l, "перемену мест").as_deref(), Some("track:Перемена Мест"));
        assert_eq!(f(&l, "гавайскую музыку").as_deref(), Some("playlist:гавайская музыка"));
        let prefer = find(&l, "группа крови", Some(Kind::Album)).unwrap();
        assert_eq!(prefer.item.kind, Kind::Album);
    }

    #[test]
    fn not_local_is_not_found() {
        let l = lib();
        for q in ["агутина", "мадонну", "кукушку", "перемен", "нау", "ким уайлд", "liked songs"] {
            assert_eq!(f(&l, q), None, "{q}");
        }
    }

    #[test]
    fn similar_songs_when_no_exact_match() {
        let l = lib();
        // «белую розу» — точного нет, но «белую» есть в «Белая ворона»
        let s: Vec<&str> = find_similar(&l, "белую розу", 30).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(s, ["Белая ворона"]);
        // исполнитель не в счёт: «кино» есть только у артиста — по нему ничего
        assert!(find_similar(&l, "кино про любовь", 30).is_empty());
        // «звучание» не в счёт: «квин» ~ Queen только в точном поиске
        assert!(find_similar(&l, "квин рапсодия", 30).is_empty());
        // совсем ничего общего — пусто
        assert!(find_similar(&l, "агутина", 30).is_empty());
        assert!(find_similar(&l, "мадонну", 30).is_empty());
    }

    #[test]
    fn ukrainian_letters_do_not_split_words() {
        assert_eq!(norm("Ікони не плачуть"), ["икони", "не", "плачуть"]);
        assert_eq!(norm("Поїзд «Чужа любов»"), ["поизд", "чужа", "любов"]);
    }

    #[test]
    fn word_levels() {
        assert_eq!(tok_level("агаты", "агата"), 2.0);
        assert_eq!(tok_level("новых", "новые"), 2.0);
        assert_eq!(tok_level("квин", "queen"), 1.5);
        assert_eq!(tok_level("кукушку", "кошка"), 0.0);
        assert_eq!(tok_level("нау", "на"), 0.0);
        assert_eq!(skel("уайлд"), skel("wilde"));
    }
}

/// Прогон на выгрузке реальной библиотеки: LIB_DIR=… cargo test real_library -- --ignored --nocapture
/// (файлы lib_artist.json, lib_album.json, lib2_track.json, lib_playlist.json — ответы get_library).
#[cfg(test)]
mod real {
    use super::*;

    #[test]
    #[ignore]
    fn real_library() {
        let dir = std::env::var("LIB_DIR").expect("LIB_DIR");
        let load = |f: &str| -> Vec<Value> {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{f}")).unwrap()).unwrap();
            v["service_response"]["items"].as_array().unwrap().clone()
        };
        let items = build(&load("lib_artist.json"), &load("lib_album.json"), &load("lib2_track.json"), &load("lib_playlist.json"));
        let qs = std::env::var("Q").unwrap();
        for q in qs.split('|') {
            let t = std::time::Instant::now();
            let r = find(&items, q, None);
            match r {
                Some(h) => println!("{q:28} -> ТОЧНО {} «{}» [{:.1}] ({} мкс)", h.item.kind.media_type(), h.item.name, h.score, t.elapsed().as_micros()),
                None => {
                    let s = find_similar(&items, q, 30);
                    let names: Vec<String> = s.iter().take(6).map(|i| format!("{} — {}", i.name, i.extra_toks.iter().map(|t| t.raw.as_str()).collect::<Vec<_>>().join(" "))).collect();
                    println!("{q:28} -> ПОХОЖИХ {}: {names:?}", s.len());
                }
            }
        }
    }
}
