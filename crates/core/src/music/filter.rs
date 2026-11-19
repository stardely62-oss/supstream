//! Фильтр запрещённых слов для текстов песен (и позже — для озвучки донатов).
//!
//! Две категории:
//! * `hate` — оскорбления и слюры: за них Twitch банит. По умолчанию трек отклоняется.
//! * `profanity` — мат: на Twitch в целом разрешён, по умолчанию глушится.
//!
//! Нормализация ловит обходы: латиница вместо кириллицы («xyй»), цифры («п1зда»,
//! «sh1t»), звёздочки («f*ck»), растянутые буквы («хууууй»), «ё» → «е».

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Profanity,
    Hate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Слово целиком (с учётом нормализации).
    Exact,
    /// Слово начинается с шаблона (основа слова).
    Prefix,
    /// Шаблон встречается внутри слова. Только для редких сочетаний букв («пизд», «fuck»).
    Contains,
    /// Корень после стандартной приставки или без неё: «нахуй», «заебать», «съебаться».
    /// Безопаснее `Contains`: «тихую», «оскорблять», «спидран» не срабатывают.
    Root,
}

/// Приставки для `Root`. Одиночные «в»/«с» не берём: «себе», «вебка».
const ROOT_PREFIXES: &[&str] = &[
    "", "на", "по", "за", "вы", "до", "от", "отъ", "об", "объ", "раз", "разъ", "рас", "про", "пере", "при",
    "под", "подъ", "из", "изъ", "съ", "въ", "вз", "у", "о", "а", "ни", "не", "недо",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub category: Category,
    pub kind: MatchKind,
}

/// Встроенный список. Формат строки: `категория вид шаблон`.
/// Его можно дополнять своими словами в настройках музыки.
const DEFAULT_RULES: &str = "
# --- оскорбления и слюры (риск бана на Twitch) ---
hate contains nigg
hate exact niga
hate exact nigas
hate contains нигг
hate exact нигер
hate exact нигеры
hate exact нигера
hate exact нигеров
hate prefix негр
hate prefix faggot
hate exact fag
hate exact fags
hate prefix retard
hate exact tranny
hate exact trannies
hate exact kike
hate exact kikes
hate exact chink
hate exact chinks
hate exact spic
hate exact spics
hate exact coon
hate exact coons
hate exact dyke
hate exact dykes
hate root пидор
hate root пидар
hate root пидр
hate prefix педик
hate prefix педераст
hate exact хач
hate exact хачи
hate exact хачей
hate prefix хачик
hate prefix чурк
hate exact жид
hate exact жиды
hate prefix жидов
hate prefix жидяр
hate exact даун
hate exact дауны
hate prefix хохол
hate prefix хохл
hate prefix москал
hate prefix кацап

# --- мат ---
profanity root хуй
profanity root хуя
profanity root хуе
profanity root хуи
profanity root хую
profanity contains пизд
profanity root еб
profanity root бляд
profanity root блят
profanity exact бля
profanity exact сука
profanity exact суки
profanity exact суку
profanity exact сукой
profanity prefix сучк
profanity prefix сучар
profanity prefix мудак
profanity prefix мудил
profanity prefix мудач
profanity prefix залуп
profanity prefix гандон
profanity prefix гондон
profanity prefix шлюх
profanity contains fuck
profanity prefix shit
profanity exact bullshit
profanity exact horseshit
profanity exact dipshit
profanity prefix bitch
profanity prefix cunt
profanity exact dick
profanity exact dicks
profanity exact whore
profanity exact whores
profanity exact slut
profanity exact sluts
";

/// Встроенные исключения: обычные слова, которые цепляются за правила.
const DEFAULT_ALLOW: &[&str] = &["shiitake"];

pub fn default_rules() -> Vec<Rule> {
    DEFAULT_RULES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let category = match it.next()? {
                "hate" => Category::Hate,
                "profanity" => Category::Profanity,
                _ => return None,
            };
            let kind = match it.next()? {
                "exact" => MatchKind::Exact,
                "prefix" => MatchKind::Prefix,
                "contains" => MatchKind::Contains,
                "root" => MatchKind::Root,
                _ => return None,
            };
            Some(Rule { pattern: it.next()?.to_string(), category, kind })
        })
        .collect()
}

/// Скрипт, к которому приводим слово перед сравнением.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    Cyrillic,
    Latin,
}

fn script_of(pattern: &str) -> Script {
    if pattern.chars().any(|c| ('а'..='я').contains(&c) || c == 'ё') {
        Script::Cyrillic
    } else {
        Script::Latin
    }
}

/// Символ → его «канонический» вид для данного алфавита. `*` остаётся как шаблон-«любая буква».
fn canon(c: char, script: Script) -> Option<char> {
    let c = c.to_lowercase().next().unwrap_or(c);
    if c == '*' {
        return Some('*');
    }
    Some(match script {
        Script::Cyrillic => match c {
            'ё' => 'е',
            'a' | '@' | '4' => 'а',
            'b' | '6' => 'б',
            'e' | '3' => 'е',
            'k' => 'к',
            'm' => 'м',
            'h' => 'н',
            'o' | '0' => 'о',
            'p' => 'р',
            'c' => 'с',
            't' => 'т',
            'y' | 'u' => 'у',
            'x' => 'х',
            'i' | '1' | '!' => 'и',
            'n' => 'п',
            'd' => 'д',
            'z' => 'з',
            'r' => 'г',
            c if c.is_alphabetic() => c,
            _ => return None,
        },
        Script::Latin => match c {
            '@' | '4' | 'а' => 'a',
            '3' | 'е' | 'ё' => 'e',
            '0' | 'о' => 'o',
            '1' | '!' | '|' => 'i',
            '5' | '$' => 's',
            '7' => 't',
            'с' => 'c',
            'р' => 'p',
            'х' => 'x',
            'у' => 'y',
            'к' => 'k',
            'м' => 'm',
            'т' => 't',
            'в' => 'b',
            'н' => 'h',
            c if c.is_alphabetic() => c,
            _ => return None,
        },
    })
}

/// Нормализованное слово (буквы не схлопываем: «Niger» ≠ «nigger»).
fn normalize(word: &str, script: Script) -> Vec<char> {
    word.chars().filter_map(|c| canon(c, script)).collect()
}

/// Совпадает ли шаблон со словом начиная с позиции `at`. Возвращает конец совпадения.
/// Растянутые буквы («хууууй», «fuuuck») съедаются: после буквы шаблона пропускаем
/// её повторы в слове, если следующая буква шаблона другая.
fn match_at(word: &[char], pat: &[char], at: usize) -> Option<usize> {
    let mut j = at;
    for (k, &p) in pat.iter().enumerate() {
        let w = *word.get(j)?;
        if w != p && w != '*' {
            return None;
        }
        j += 1;
        if pat.get(k + 1) != Some(&p) {
            while word.get(j) == Some(&p) {
                j += 1;
            }
        }
    }
    Some(j)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hit {
    /// Позиция слова в строке (в символах) — для подсветки и оценки времени.
    pub start: usize,
    pub end: usize,
    pub word: String,
    pub category: Category,
}

struct Compiled {
    pattern: Vec<char>,
    script: Script,
    /// Для `Root`: нормализованные приставки.
    prefixes: Vec<Vec<char>>,
    category: Category,
    kind: MatchKind,
}

pub struct Filter {
    rules: Vec<Compiled>,
    /// Исключения: слова, которые не считаются нарушением (нормализованные в обоих алфавитах).
    allow: Vec<(Vec<char>, Vec<char>)>,
}

impl Filter {
    pub fn new(rules: impl IntoIterator<Item = Rule>, allow: &[String]) -> Self {
        let rules = rules
            .into_iter()
            .filter(|r| !r.pattern.trim().is_empty())
            .map(|r| {
                let script = script_of(&r.pattern.to_lowercase());
                Compiled {
                    pattern: normalize(&r.pattern, script),
                    prefixes: if r.kind == MatchKind::Root {
                        ROOT_PREFIXES.iter().map(|p| normalize(p, script)).collect()
                    } else {
                        Vec::new()
                    },
                    script,
                    category: r.category,
                    kind: r.kind,
                }
            })
            .collect();
        let allow =
            allow.iter().map(|w| (normalize(w, Script::Cyrillic), normalize(w, Script::Latin))).collect();
        Self { rules, allow }
    }

    /// Встроенные правила + свои слова из настроек.
    pub fn with_custom(extra: &[Rule], allow: &[String]) -> Self {
        let allow: Vec<String> =
            DEFAULT_ALLOW.iter().map(|s| s.to_string()).chain(allow.iter().cloned()).collect();
        Self::new(default_rules().into_iter().chain(extra.iter().cloned()), &allow)
    }

    fn check_word(&self, word: &str) -> Option<Category> {
        let cyr = normalize(word, Script::Cyrillic);
        let lat = normalize(word, Script::Latin);
        if self.allow.iter().any(|(c, l)| *c == cyr || *l == lat) {
            return None;
        }
        let mut worst: Option<Category> = None;
        for r in &self.rules {
            let w = if r.script == Script::Cyrillic { &cyr } else { &lat };
            let hit = match r.kind {
                MatchKind::Exact => match_at(w, &r.pattern, 0) == Some(w.len()),
                MatchKind::Prefix => match_at(w, &r.pattern, 0).is_some(),
                MatchKind::Contains => (0..w.len()).any(|i| match_at(w, &r.pattern, i).is_some()),
                MatchKind::Root => r.prefixes.iter().any(|pre| {
                    // Приставка должна быть настоящей, без «*»-подстановок.
                    w.len() >= pre.len()
                        && w[..pre.len()] == pre[..]
                        && match_at(w, &r.pattern, pre.len()).is_some()
                }),
            };
            // Слово из одних звёздочек совпало бы с чем угодно.
            if hit && w.iter().any(|c| *c != '*') {
                worst = worst.max(Some(r.category));
            }
        }
        worst
    }

    /// Все нарушения в строке.
    pub fn scan(&self, line: &str) -> Vec<Hit> {
        let chars: Vec<char> = line.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || matches!(c, '*' | '@' | '$' | '!' | '|');
        let mut hits = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if !is_word(chars[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            // `!` и `|` в конце — это пунктуация, а не замена буквы.
            let mut end = i;
            while end > start && matches!(chars[end - 1], '!' | '|') {
                end -= 1;
            }
            let word: String = chars[start..end].iter().collect();
            if let Some(category) = self.check_word(&word) {
                hits.push(Hit { start, end, word, category });
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> Filter {
        Filter::with_custom(&[], &[])
    }

    fn cats(line: &str) -> Vec<(String, Category)> {
        f().scan(line).into_iter().map(|h| (h.word, h.category)).collect()
    }

    #[test]
    fn default_rules_parse() {
        let rules = default_rules();
        assert!(rules.len() > 60);
        assert!(rules.iter().any(|r| r.category == Category::Hate));
    }

    #[test]
    fn russian_mat_with_prefixes_and_forms() {
        for w in [
            "нахуй",
            "Хуёво",
            "охуеть",
            "нихуя",
            "распиздяй",
            "заебали",
            "ебать",
            "съебался",
            "отъебись",
            "блядь",
            "выблядок",
            "Сука",
            "мудак",
            "уебан",
            "ёбаный",
        ] {
            assert_eq!(cats(w), vec![(w.to_string(), Category::Profanity)], "{w}");
        }
    }

    #[test]
    fn obfuscation_is_caught() {
        for w in ["xyй", "п1зда", "хууууй", "f*ck", "sh1t", "FUUUCK", "6лядь", "пиzда", "niiiggaa", "з@ебал"]
        {
            assert_eq!(f().scan(w).len(), 1, "не поймано: {w}");
        }
    }

    #[test]
    fn hate_beats_profanity_and_is_found_in_sentence() {
        let hits = f().scan("Эй, ты, пидорас, иди отсюда!");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].category, Category::Hate);
        assert_eq!(hits[0].word, "пидорас");
        let line = "Эй, ты, пидорас, иди отсюда!";
        let chars: Vec<char> = line.chars().collect();
        assert_eq!(chars[hits[0].start..hits[0].end].iter().collect::<String>(), "пидорас");
        assert_eq!(cats("my nigga")[0].1, Category::Hate);
        assert_eq!(cats("RETARDED")[0].1, Category::Hate);
    }

    #[test]
    fn no_false_positives_on_common_words() {
        let clean = [
            "Ребята, тебе хуже без меня", // «хуже», «ребята», «тебе»
            "Мебель, корабль, оскорблять, употреблять",
            "сукно, сукровица, хачапури, жидкий, чурбан",
            "Скандинавия, рубля, колебаться, хлебать",
            "raccoon, cocktail, Scunthorpe hello, assassin, shiitake",
            "Do you know the way, she sells sea shells",
            "небо, хлеб, требовать, гребать",
            "тихую, сухую, глухую, лихую, психует, психуют, худой, хулиган",
            "колебаться, хлебать, оскорблять, употреблять, себе, вебка, тебе",
            "спидран, спидраннер, Nigeria, Niger, night, nightmare",
        ];
        for line in clean {
            assert!(f().scan(line).is_empty(), "ложное срабатывание: {line} → {:?}", f().scan(line));
        }
    }

    #[test]
    fn punctuation_and_exclamation_are_not_letters() {
        assert_eq!(cats("сука!"), vec![("сука".into(), Category::Profanity)]);
        assert!(f().scan("!!! ***").is_empty());
    }

    #[test]
    fn allowlist_and_custom_rules() {
        let custom =
            vec![Rule {
                pattern: "кринж".into(), category: Category::Profanity, kind: MatchKind::Prefix
            }];
        let flt = Filter::with_custom(&custom, &["сука".to_string()]);
        assert!(flt.scan("сука").is_empty());
        assert_eq!(flt.scan("кринжово").len(), 1);
    }
}
