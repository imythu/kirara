mod semantic;
// Search uses local rows as candidates; public metadata never creates a local identity.
use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default)]
pub struct SearchDocument {
    pub id: i64,
    pub names: Vec<String>,
    pub fields: Vec<String>,
    pub site_names: Vec<String>,
    pub site_fields: Vec<String>,
    pub catalog_id: Option<String>,
    pub site_id: Option<i64>,
    pub enabled: Option<bool>,
    pub result: Option<String>,
    pub health: Option<String>,
    pub site_type: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct SearchFilters {
    pub id: Option<i64>,
    pub site_id: Option<i64>,
    pub enabled: Option<bool>,
    pub result: Option<String>,
    pub health: Option<String>,
    pub site_type: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ParsedFilter {
    pub field: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    #[serde(skip)]
    pub rank: u8,
    #[serde(skip)]
    pub similarity: f32,
    pub id: i64,
    pub matched_by: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SearchOutput {
    pub hits: Vec<SearchHit>,
    pub parsed_filters: Vec<ParsedFilter>,
    pub semantic_status: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub aka: Vec<String>,
    pub url: String,
    pub hosts: Vec<String>,
    pub ptd_ids: Vec<String>,
    pub categories: Vec<String>,
    pub content_types: Vec<String>,
    pub specialties: Vec<String>,
    pub official_groups: Vec<String>,
    pub community_labels: Vec<String>,
    pub description: String,
    pub resource_text: String,
    pub feature_text: String,
}
fn parse_catalog(json: &str) -> Result<Vec<CatalogEntry>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    serde_json::from_value(value.get("sites").cloned().unwrap_or(value))
}
static CATALOG: LazyLock<Vec<CatalogEntry>> = LazyLock::new(|| {
    parse_catalog(include_str!("../../assets/search/catalog.json")).unwrap_or_else(|_| {
        tracing::error!("Search catalog is invalid; public search metadata unavailable");
        Vec::new()
    })
});
pub fn catalog() -> &'static [CatalogEntry] {
    &CATALOG
}
pub fn catalog_revision() -> &'static str {
    static REVISION: LazyLock<String> = LazyLock::new(|| {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/search/catalog.json"))
                .unwrap_or_default();
        value["source_revision"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned()
    });
    &REVISION
}
#[cfg(test)]
pub fn catalog_id_for_ptd_id(id: &str) -> Option<String> {
    static CROSSWALK: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../assets/search/ptd-crosswalk.json"))
            .unwrap_or_default()
    });
    CROSSWALK.get(id).cloned()
}
pub fn normalize_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
}
pub fn resolve_catalog(url: &str, mode: &str, manual_id: Option<&str>) -> Option<String> {
    match mode {
        "none" => None,
        // Keep unresolved manual IDs so catalog upgrades never silently rebind them.
        "manual" => manual_id.map(str::to_owned),
        "auto" => {
            let host = normalize_host(url)?;
            static HOSTS: LazyLock<HashMap<String, Option<String>>> = LazyLock::new(|| {
                let mut hosts: HashMap<String, Option<String>> = HashMap::new();
                for entry in catalog() {
                    for host in entry
                        .hosts
                        .iter()
                        .cloned()
                        .chain(normalize_host(&entry.url))
                    {
                        hosts
                            .entry(host)
                            .and_modify(|id| {
                                if id.as_deref() != Some(&entry.id) {
                                    *id = None;
                                }
                            })
                            .or_insert_with(|| Some(entry.id.clone()));
                    }
                }
                hosts
            });
            HOSTS.get(&host).cloned().flatten()
        }
        _ => None,
    }
}
pub fn normalize(value: &str) -> String {
    let canonical: String = value.nfkc().collect();
    icu_casemap::CaseMapper::new()
        .fold_string(&canonical)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("u:", "v")
        .replace('ü', "v")
}
fn compact(value: &str) -> String {
    value.chars().filter(|c| c.is_alphanumeric()).collect()
}
#[derive(Debug, PartialEq, Eq)]
struct Text {
    plain: String,
    compact: String,
    pinyin: String,
    initials: String,
}
static TEXT_CACHE: LazyLock<RwLock<HashMap<String, Arc<Text>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
fn text(value: &str) -> Arc<Text> {
    if value.len() <= 512 {
        let cache = TEXT_CACHE.read().unwrap_or_else(|e| e.into_inner());
        if let Some(text) = cache.get(value) {
            return Arc::clone(text);
        }
    }
    // Miss computation does not serialize other readers or unrelated misses.
    let text = Arc::new(build_text(value));
    if value.len() > 512 {
        return text;
    }
    let mut cache = TEXT_CACHE.write().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = cache.get(value) {
        return Arc::clone(existing);
    }
    if cache.len() >= 4096 {
        cache.clear();
    }
    cache.insert(value.to_owned(), Arc::clone(&text));
    text
}
fn build_text(value: &str) -> Text {
    let plain = normalize(value);
    let mut pinyin = String::new();
    let mut initials = String::new();
    // Fixed common site-name polyphone corrections, without combinatorial expansion.
    let corrected = plain.replace("重庆", "重慶");
    for (i, c) in corrected.chars().enumerate() {
        let py = if c == '重' && corrected.chars().nth(i + 1) == Some('慶') {
            Some("chong")
        } else if c == '乐' && i > 0 && corrected.chars().nth(i - 1) == Some('音') {
            Some("yue")
        } else {
            c.to_pinyin().map(|p| p.plain())
        };
        if let Some(py) = py {
            pinyin.push_str(py);
            initials.push(py.chars().next().unwrap());
        } else if c.is_alphanumeric() {
            pinyin.push(c);
            initials.push(c);
        }
    }
    Text {
        compact: compact(&plain),
        plain,
        pinyin,
        initials,
    }
}
fn expanded_public_text(entry: &CatalogEntry) -> String {
    normalize(&search_encoder::expand_semantic_text(&format!(
        "{} {} {} {} {}",
        entry.resource_text,
        entry.feature_text,
        entry.description,
        entry.categories.join(" "),
        entry.content_types.join(" ")
    )))
}
fn public_description(id: &str) -> Option<&'static str> {
    static DESCRIPTIONS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
        catalog()
            .iter()
            .map(|entry| (entry.id.clone(), expanded_public_text(entry)))
            .collect()
    });
    DESCRIPTIONS.get(id).map(String::as_str)
}

#[derive(Clone, Debug)]
struct Term {
    value: String,
    field: Option<String>,
    quoted: bool,
}
fn tokenize(q: &str) -> Result<Vec<Term>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut had_quote = false;
    for c in q.chars() {
        if c == '"' {
            quoted = !quoted;
            had_quote = true;
        } else if c.is_whitespace() && !quoted {
            if !current.is_empty() {
                tokens.push((std::mem::take(&mut current), had_quote));
                had_quote = false;
            }
        } else {
            current.push(c);
        }
    }
    if quoted {
        return Err("Unclosed quote".into());
    }
    if !current.is_empty() {
        tokens.push((current, had_quote));
    }
    tokens
        .into_iter()
        .map(|(s, quoted)| {
            let (field, value) = if let Some((f, v)) = s.split_once(':') {
                if !["name", "site", "id", "enabled", "result", "health", "type"].contains(&f) {
                    return Err(format!("Unsupported search field: {f}"));
                }
                if v.is_empty() {
                    return Err(format!("Empty search field: {f}"));
                }
                (Some(f.to_owned()), v.to_owned())
            } else {
                (None, s)
            };
            Ok(Term {
                value,
                field,
                quoted,
            })
        })
        .collect()
}
// Sentence punctuation stays in the encoded clause; only grammar recognition
// uses the word form. Interior dots and digits remain domain/ID literals.
fn trim_sentence_punctuation(value: &str) -> &str {
    value.trim_matches(|c: char| {
        matches!(
            c,
            '.' | ','
                | '?'
                | '!'
                | ';'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '…'
                | '。'
                | '？'
                | '！'
                | '，'
                | '；'
        )
    })
}
fn prose_word(value: &str) -> Option<&str> {
    let word = trim_sentence_punctuation(value);
    (word.chars().any(|c| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, '\'' | '’' | '-')))
    .then_some(word)
}
fn merge_concept_phrases(terms: &mut Vec<Term>) {
    let mut index = 0;
    while index < terms.len() {
        let limit = (terms.len() - index).min(semantic::max_phrase_words());
        let mut merged = None;
        for length in (2..=limit).rev() {
            let span = &terms[index..index + length];
            if span.iter().any(|t| t.field.is_some() || t.quoted) {
                continue;
            }
            let phrase = span
                .iter()
                .map(|t| t.value.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if semantic::exact_multiword_concept(&phrase) {
                merged = Some((length, phrase));
                break;
            }
        }
        if let Some((length, phrase)) = merged {
            terms.splice(
                index..index + length,
                [Term {
                    value: phrase,
                    field: None,
                    quoted: false,
                }],
            );
        }
        index += 1;
    }
}
fn prose_function_word(word: &str) -> bool {
    [
        "i", "me", "my", "mine", "we", "us", "our", "you", "your", "he", "him", "his", "she",
        "her", "it", "its", "they", "them", "their", "a", "an", "the", "for", "of", "with", "and",
        "to", "in", "on", "at", "by", "from", "about", "that", "which", "who", "what", "where",
        "can", "could", "would", "should", "is", "are", "be", "has", "have", "do", "does",
    ]
    .contains(&word)
}
fn split_description(
    terms: &mut Vec<Term>,
    whole_name: bool,
    mut known_name: impl FnMut(&Term) -> bool,
) -> Vec<String> {
    let has_concept = terms
        .iter()
        .any(|t| t.field.is_none() && !t.quoted && !semantic::expansions(&t.value).is_empty());
    let english_prose = terms
        .iter()
        .filter(|t| t.field.is_none() && !t.quoted)
        .flat_map(|t| t.value.split_whitespace())
        .filter(|word| prose_word(word).is_some())
        .count()
        >= 3;
    let modifiers = [
        "good",
        "best",
        "suitable",
        "friendly",
        "beginner",
        "beginners",
        "rare",
        "arthouse",
        "high",
        "quality",
        "focused",
        "specialized",
        "classic",
        "international",
        "new",
        "users",
        "content",
        "resources",
        "site",
        "sites",
        "tracker",
        "trackers",
    ];
    let mut description = Vec::new();
    terms.retain(|term| {
        let word=prose_word(&term.value);
        let negated=word.is_some_and(|w|["not","no","without","exclude","never","don't","dont"].contains(&w))
            || ["未","不要","不","没有","无关"].iter().any(|prefix|term.value.starts_with(prefix));
        let descriptive = !whole_name && term.field.is_none() && !term.quoted && !negated
            && (!semantic::expansions(&term.value).is_empty()
                || (has_concept && word.is_some_and(|w|prose_function_word(w)||modifiers.contains(&w)))
                || (english_prose && word.is_some()))
            // Ordinary grammar in a sentence is not an entity clue just because
            // a catalog happens to have an alias such as ME, IT, or US.
            && ((english_prose && word.is_some_and(prose_function_word)) || !known_name(term));
        if descriptive {description.push(term.value.clone());}
        !descriptive
    });
    description
}
fn collection_aliases() -> &'static HashMap<String, String> {
    static LABELS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
        let mut labels: HashMap<String, String> = catalog()
            .iter()
            .flat_map(|e| e.community_labels.iter())
            .map(|s| (normalize(s), s.clone()))
            .collect();
        for (alias, label) in [
            ("top 12 chinese trackers", "十二大"),
            ("traditional nine", "传统九大"),
            ("top movie tv trackers", "外站影视三强"),
        ] {
            if labels.contains_key(&normalize(label)) {
                labels.insert(alias.into(), label.into());
            }
        }
        labels
    });
    &LABELS
}
fn extract_collections(terms: &mut Vec<Term>, whole_name: bool, parsed: &mut Vec<ParsedFilter>) {
    if whole_name {
        return;
    }
    let mut index = 0;
    while index < terms.len() {
        let mut found = None;
        for length in (1..=(terms.len() - index).min(5)).rev() {
            let span = &terms[index..index + length];
            if span.iter().any(|t| t.field.is_some() || t.quoted) {
                continue;
            }
            let phrase = span
                .iter()
                .map(|t| t.value.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let phrase = phrase
                .trim_matches(|c| matches!(c, '?' | '？' | '!' | '！' | '.' | '。' | ',' | '，'));
            if let Some(label) = collection_aliases().get(phrase) {
                found = Some((length, label.clone()));
                break;
            }
        }
        if let Some((length, label)) = found {
            parsed.push(ParsedFilter {
                field: "collection".into(),
                value: label,
            });
            terms.drain(index..index + length);
        } else {
            index += 1;
        }
    }
}
fn one_error(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut i = 0;
    while i < a.len().min(b.len()) && a[i] == b[i] {
        i += 1;
    }
    if i == a.len().min(b.len()) {
        return true;
    }
    if a.len() == b.len() {
        a[i + 1..] == b[i + 1..]
            || (i + 1 < a.len() && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..])
    } else if a.len() > b.len() {
        a[i + 1..] == b[i..]
    } else {
        a[i..] == b[i + 1..]
    }
}
fn matches(t: &Text, term: &Term, name: bool, official: bool) -> Option<(u8, &'static str)> {
    let q = &term.value;
    let c = compact(q);
    if t.plain == *q || (!term.quoted && t.compact == c) {
        return Some((
            if name {
                if official { 1 } else { 0 }
            } else {
                3
            },
            if official {
                "catalog_alias"
            } else if name {
                "name"
            } else {
                "text"
            },
        ));
    }
    if term.quoted {
        return t.plain.contains(q).then_some((3, "text"));
    }
    if c.is_empty() {
        return None;
    }
    if q.chars().count() >= 2 && t.compact.starts_with(&c) {
        return Some((2, if name { "name" } else { "text" }));
    }
    if q.chars().count() >= 2 && t.plain.contains(q) {
        return Some((3, "text"));
    }
    if name && c.len() >= 2 && t.pinyin == c {
        return Some((2, "pinyin"));
    }
    if name && c.len() >= 2 && t.initials == c {
        return Some((4, "initials"));
    }
    if !term.quoted && q.len() >= 4 && q.bytes().all(|b| b.is_ascii_alphabetic()) {
        if (name && one_error(q, &t.pinyin))
            || t.plain
                .split(|c: char| !c.is_ascii_alphabetic())
                .filter(|s| s.len() >= 4)
                .any(|s| one_error(q, s))
        {
            return Some((4, "fuzzy"));
        }
    }
    None
}
fn filter_value(doc: &SearchDocument, field: &str, value: &str) -> bool {
    match field {
        "id" => value.parse::<i64>().ok() == Some(doc.id),
        "site" => value
            .parse::<i64>()
            .ok()
            .is_some_and(|id| doc.site_id == Some(id)),
        "enabled" => doc.enabled == value.parse::<bool>().ok(),
        "result" => doc.result.as_deref() == Some(value),
        "health" => doc.health.as_deref() == Some(value),
        "type" => doc.site_type.as_deref().map(normalize).as_deref() == Some(value),
        "collection" => doc
            .catalog_id
            .as_deref()
            .and_then(|id| catalog().iter().find(|e| e.id == id))
            .is_some_and(|e| e.community_labels.iter().any(|label| label == value)),
        _ => false,
    }
}
fn validate_filter(field: &str, value: &str) -> Result<(), String> {
    let valid = match field {
        "id" | "site" => value.parse::<i64>().is_ok(),
        "enabled" => ["true", "false"].contains(&value),
        "result" => ["success", "failed", "unknown"].contains(&value),
        "health" => ["healthy", "failed", "pending"].contains(&value),
        "type" => !value.is_empty(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("Invalid {field} filter: {value}"))
    }
}
#[derive(Default, Clone)]
pub struct SearchContext {
    pub exact_name: bool,
    pub literal_terms: BTreeSet<String>,
}
pub struct PreparedSearch {
    q: String,
    filters: SearchFilters,
    context: SearchContext,
    semantic: OnceLock<(String, Vec<(String, f32)>)>,
}
pub fn context_census_queries(q: &str) -> Result<Vec<String>, String> {
    if q.chars().count() > 256 {
        return Err("Search query exceeds 256 characters".into());
    }
    let q = normalize(q);
    let mut terms = vec![q.clone()];
    // A literal complete name may itself contain punctuation resembling query syntax.
    if let Ok(mut tokens) = tokenize(&q) {
        merge_concept_phrases(&mut tokens);
        terms.extend(tokens.into_iter().map(|t| t.value));
    }
    terms.sort();
    terms.dedup();
    Ok(terms)
}
pub fn name_matches(name: &str, query: &str) -> bool {
    !query.is_empty() && compact(&normalize(name)) == compact(&normalize(query))
}
fn whole_query_name_matches(name: &str, query: &str) -> bool {
    if name_matches(name, query) {
        return true;
    }
    // Spacing may segment a complete phonetic name; never OR-match individual
    // syllables, and never reinterpret quoted/explicit query syntax this way.
    let query = normalize(query);
    if !query.chars().any(char::is_whitespace)
        || !query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
    {
        return false;
    }
    let phonetic = compact(&query);
    if phonetic.len() < 2 {
        return false;
    }
    let name = text(name);
    name.pinyin == phonetic || name.initials == phonetic
}
pub fn context_observe(
    context: &mut SearchContext,
    docs: &[SearchDocument],
    q: &str,
) -> Result<(), String> {
    let queries = context_census_queries(q)?;
    for doc in docs {
        let entry = doc
            .catalog_id
            .as_deref()
            .and_then(|id| catalog().iter().find(|e| e.id == id));
        let names: Vec<&str> = doc
            .names
            .iter()
            .chain(doc.site_names.iter())
            .map(String::as_str)
            .chain(entry.into_iter().flat_map(|e| {
                std::iter::once(e.name.as_str()).chain(e.aka.iter().map(String::as_str))
            }))
            .collect();
        for query in &queries {
            if names.iter().any(|n| {
                if *query == normalize(q) {
                    whole_query_name_matches(n, query)
                } else {
                    name_matches(n, query)
                }
            }) {
                context.literal_terms.insert(query.clone());
                if *query == normalize(q) {
                    context.exact_name = true;
                }
            }
        }
    }
    Ok(())
}
pub fn prepare_context(
    q: &str,
    filters: &SearchFilters,
    context: SearchContext,
) -> Result<PreparedSearch, String> {
    let prepared = PreparedSearch {
        q: q.into(),
        filters: filters.clone(),
        context,
        semantic: OnceLock::new(),
    };
    search_internal(&[], q, filters, Some(&prepared))?;
    Ok(prepared)
}
pub fn search_prepared(
    docs: &[SearchDocument],
    prepared: &PreparedSearch,
) -> Result<SearchOutput, String> {
    search_internal(docs, &prepared.q, &prepared.filters, Some(prepared))
}
pub fn search(
    docs: &[SearchDocument],
    q: &str,
    filters: &SearchFilters,
) -> Result<SearchOutput, String> {
    search_internal(docs, q, filters, None)
}
// Entity/literal tiers are independent of the descriptive evidence channel.
// Pure descriptions share one tier: category containment cannot override a
// closer semantic match, and exact entities cannot be demoted by soft evidence.
fn result_tier(literal_rank: u8, has_literals: bool, has_description: bool) -> u8 {
    if has_description && !has_literals {
        5
    } else {
        literal_rank
    }
}
fn compare_hits(a: &SearchHit, b: &SearchHit) -> std::cmp::Ordering {
    a.rank
        .cmp(&b.rank)
        .then_with(|| b.similarity.total_cmp(&a.similarity))
        .then_with(|| a.id.cmp(&b.id))
}
fn search_internal(
    docs: &[SearchDocument],
    q: &str,
    filters: &SearchFilters,
    prepared: Option<&PreparedSearch>,
) -> Result<SearchOutput, String> {
    if q.chars().count() > 256 {
        return Err("Search query exceeds 256 characters".into());
    }
    let q = normalize(q);
    let mut parsed = Vec::<ParsedFilter>::new();
    for (field, value) in [
        ("id", filters.id.map(|v| v.to_string())),
        ("site", filters.site_id.map(|v| v.to_string())),
        ("enabled", filters.enabled.map(|v| v.to_string())),
        ("result", filters.result.clone()),
        ("health", filters.health.clone()),
        ("type", filters.site_type.clone()),
    ] {
        if let Some(value) = value {
            validate_filter(field, &value)?;
            parsed.push(ParsedFilter {
                field: field.into(),
                value,
            });
        }
    }
    let entries: HashMap<_, _> = catalog().iter().map(|e| (e.id.as_str(), e)).collect();
    let whole_name = prepared.map(|p| p.context.exact_name).unwrap_or_else(|| {
        !q.is_empty()
            && docs.iter().any(|d| {
                d.names
                    .iter()
                    .chain(d.site_names.iter())
                    .any(|n| whole_query_name_matches(n, &q))
                    || d.catalog_id
                        .as_deref()
                        .and_then(|id| entries.get(id))
                        .is_some_and(|e| {
                            std::iter::once(&e.name)
                                .chain(e.aka.iter())
                                .any(|n| whole_query_name_matches(n, &q))
                        })
            })
    });
    let mut terms = Vec::new();
    let mut tokens = if whole_name {
        vec![Term {
            value: q.clone(),
            field: Some("name".into()),
            quoted: false,
        }]
    } else {
        tokenize(&q)?
    };
    merge_concept_phrases(&mut tokens);
    for term in tokens {
        if let Some(f) = term.field.as_deref() {
            if f != "name" {
                if f == "site" && term.value.parse::<i64>().is_err() {
                    terms.push(term);
                    continue;
                }
                validate_filter(f, &term.value)?;
                parsed.push(ParsedFilter {
                    field: f.into(),
                    value: term.value,
                });
                continue;
            }
        }
        if term.field.is_none() && !term.quoted {
            let natural = match term.value.as_str() {
                "失败" | "failed" => Some(("result", "failed")),
                "成功" => Some(("result", "success")),
                "已暂停" | "已停用" => Some(("enabled", "false")),
                "已启用" => Some(("enabled", "true")),
                "待采集" => Some(("health", "pending")),
                "正常" => Some(("health", "healthy")),
                _ => None,
            };
            if let Some((field, value)) = natural {
                parsed.push(ParsedFilter {
                    field: field.into(),
                    value: value.into(),
                });
                continue;
            }
        }
        terms.push(term);
    }
    extract_collections(&mut terms, whole_name, &mut parsed);
    let semantic_terms = split_description(&mut terms, whole_name, |term| {
        prepared
            .map(|p| p.context.literal_terms.contains(&term.value))
            .unwrap_or_else(|| {
                docs.iter().any(|d| {
                    d.names
                        .iter()
                        .chain(d.site_names.iter())
                        .any(|n| name_matches(n, &term.value))
                        || d.catalog_id
                            .as_deref()
                            .and_then(|id| entries.get(id))
                            .is_some_and(|e| {
                                std::iter::once(&e.name)
                                    .chain(e.aka.iter())
                                    .any(|n| name_matches(n, &term.value))
                            })
                })
            })
    });
    let semantic_query = semantic_terms.join(" ");
    let (mut semantic_status, scores) = if semantic_query.is_empty() {
        ("not_needed".into(), Vec::new())
    } else {
        if let Some(prepared) = prepared {
            prepared
                .semantic
                .get_or_init(|| semantic::scores(&semantic_query))
                .clone()
        } else {
            semantic::scores(&semantic_query)
        }
    };
    if catalog().is_empty() || !semantic::concepts_available() {
        semantic_status = "unavailable".into();
    }
    let scores: HashMap<_, _> = scores.into_iter().collect();
    let concept_expansions: Vec<_> = semantic::expansions(&semantic_query)
        .into_iter()
        .map(|concept| normalize(&concept))
        .collect();
    let mut ranked = Vec::new();
    for doc in docs {
        if !parsed.iter().all(|f| filter_value(doc, &f.field, &f.value)) {
            continue;
        }
        if terms.is_empty() && semantic_query.is_empty() {
            ranked.push((
                0,
                0.0,
                SearchHit {
                    id: doc.id,
                    rank: 0,
                    similarity: 0.0,
                    matched_by: if parsed.iter().any(|f| f.field == "collection") {
                        vec!["collection".into()]
                    } else {
                        Vec::new()
                    },
                },
            ));
            continue;
        }
        let entry = doc
            .catalog_id
            .as_deref()
            .and_then(|id| entries.get(id))
            .copied();
        let mut fields: Vec<(Arc<Text>, bool, bool)> =
            doc.names.iter().map(|n| (text(n), true, false)).collect();
        fields.extend(doc.fields.iter().map(|s| (text(s), false, false)));
        fields.extend(doc.site_names.iter().map(|s| (text(s), true, false)));
        fields.extend(doc.site_fields.iter().map(|s| (text(s), false, false)));
        let mut site_fields: Vec<(Arc<Text>, bool, bool)> = doc
            .site_names
            .iter()
            .map(|s| (text(s), true, false))
            .chain(doc.site_fields.iter().map(|s| (text(s), false, false)))
            .collect();
        if let Some(e) = entry {
            site_fields.extend(
                std::iter::once(&e.name)
                    .chain(e.aka.iter())
                    .map(|s| (text(s), true, true)),
            );
            site_fields.extend(e.hosts.iter().map(|s| (text(s), false, true)));
        }

        if let Some(e) = entry {
            fields.extend(
                std::iter::once(&e.name)
                    .chain(e.aka.iter())
                    .map(|s| (text(s), true, true)),
            );
            fields.extend(
                e.hosts
                    .iter()
                    .chain(e.official_groups.iter())
                    .chain(e.categories.iter())
                    .chain(e.content_types.iter())
                    .chain(e.specialties.iter())
                    .chain(e.community_labels.iter())
                    .map(|s| (text(s), false, true)),
            );
        }
        let mut rank = 0;
        let mut reasons = BTreeSet::new();
        if parsed.iter().any(|f| f.field == "collection") {
            reasons.insert("collection".into());
        }
        let mut accepted = true;
        for term in &terms {
            let selected = if term.field.as_deref() == Some("site") {
                &site_fields
            } else {
                &fields
            };
            let hit = selected
                .iter()
                .filter(|(_, name, _)| match term.field.as_deref() {
                    Some("name") => *name,
                    Some("site") => true,
                    _ => true,
                })
                .filter_map(|(t, name, official)| matches(t, term, *name, *official))
                .min_by_key(|h| h.0);
            if let Some((r, why)) = hit {
                rank = rank.max(r);
                reasons.insert(why.to_owned());
            } else {
                accepted = false;
                break;
            }
        }
        let mut similarity = 0.0_f32;
        if accepted && !semantic_query.is_empty() {
            similarity = doc
                .catalog_id
                .as_ref()
                .and_then(|id| scores.get(id))
                .copied()
                .unwrap_or(0.);
            let concept_match = !concept_expansions.is_empty()
                && entry
                    .and_then(|e| public_description(&e.id))
                    .is_some_and(|public| {
                        concept_expansions
                            .iter()
                            .all(|concept| public.contains(concept))
                    });
            let literal = semantic_terms.iter().all(|value| {
                fields.iter().any(|(t, _, _)| {
                    matches(
                        t,
                        &Term {
                            value: value.clone(),
                            field: None,
                            quoted: true,
                        },
                        false,
                        false,
                    )
                    .is_some()
                })
            });
            if concept_match {
                reasons.insert("concept".into());
            } else if literal {
                reasons.insert("text".into());
            } else if similarity >= 0.65 {
                reasons.insert("semantic".into());
            } else {
                accepted = false;
            }
        }
        if accepted {
            rank = result_tier(rank, !terms.is_empty(), !semantic_query.is_empty());
            ranked.push((
                rank,
                similarity,
                SearchHit {
                    rank,
                    similarity,
                    id: doc.id,
                    matched_by: reasons.into_iter().collect(),
                },
            ));
        }
    }
    if !q.is_empty() {
        ranked.sort_by(|a, b| compare_hits(&a.2, &b.2));
    }
    Ok(SearchOutput {
        hits: ranked.into_iter().map(|(_, _, h)| h).collect(),
        parsed_filters: parsed,
        semantic_status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc(id: i64, name: &str) -> SearchDocument {
        SearchDocument {
            id,
            names: vec![name.into()],
            ..Default::default()
        }
    }
    fn ids(docs: &[SearchDocument], q: &str) -> Vec<i64> {
        search(docs, q, &SearchFilters::default())
            .unwrap()
            .hits
            .iter()
            .map(|h| h.id)
            .collect()
    }
    #[test]
    fn fixed_lexical_corpus() {
        #[derive(Deserialize)]
        struct Case {
            name: String,
            query: String,
            expected: bool,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../tests/fixtures/search/lexical.json")).unwrap();
        assert!(cases.len() >= 100);
        for case in cases {
            assert_eq!(
                !ids(&[doc(1, &case.name)], &case.query).is_empty(),
                case.expected,
                "name={} query={}",
                case.name,
                case.query
            );
        }
    }
    #[test]
    fn current_names_and_pinyin() {
        let mut docs = vec![doc(2, "备用小站"), doc(1, "失败收藏站")];
        docs[1].result = Some("success".into());
        for q in [
            "备用小站",
            "beiyongxiaozhan",
            "byxz",
            "ＢＥＩＹＯＮＧＸＩＡＯＺＨＡＮ",
        ] {
            assert_eq!(ids(&docs, q), vec![2], "{q}");
        }
        assert_eq!(ids(&docs, "失败收藏站"), vec![1]);
        assert_eq!(ids(&docs, "name:\"失败收藏站\""), vec![1]);
        assert!(ids(&docs, "result:failed").is_empty());
        docs[0].names = vec!["新名称".into()];
        assert!(ids(&docs, "备用小站").is_empty());
    }
    #[test]
    fn and_status_and_duplicates() {
        let mut a = doc(2, "青蛙 备用");
        a.result = Some("failed".into());
        a.enabled = Some(true);
        let mut b = a.clone();
        b.id = 1;
        b.result = Some("success".into());
        assert_eq!(ids(&[a.clone(), b.clone()], "青蛙"), vec![1, 2]);
        assert_eq!(ids(&[a.clone(), b.clone()], "青蛙 失败"), vec![2]);
        assert!(ids(&[a.clone()], "青蛙 已暂停").is_empty());
        assert!(ids(&[a.clone()], "青蛙 丢失").is_empty());
        assert!(
            search(
                &[a],
                "失败",
                &SearchFilters {
                    result: Some("success".into()),
                    ..Default::default()
                }
            )
            .unwrap()
            .hits
            .is_empty()
        );
    }
    #[test]
    fn typo_short_negative_and_order() {
        let docs = vec![doc(9, "QingWa"), doc(3, "M-Team"), doc(1, "QingWa")];
        assert_eq!(ids(&docs, "qignwa"), vec![1, 9]);
        assert_eq!(ids(&docs, "Ｍ ＴＥＡＭ"), vec![3]);
        for q in [
            "q",
            "qw",
            "未失败",
            "不要失败",
            "asdfqzxcv",
            "qingwa.invalid",
        ] {
            assert!(ids(&docs, q).is_empty(), "{q}");
        }
        assert_eq!(ids(&docs, ""), vec![9, 3, 1]);
        assert!(search(&docs, "foo:bar", &Default::default()).is_err());
    }
    #[test]
    fn all_candidates_before_paging() {
        let docs: Vec<_> = (0..125).map(|id| doc(id, "备用小站")).collect();
        let output = search(&docs, "byxz", &Default::default()).unwrap();
        assert_eq!(output.hits.len(), 125);
        assert_eq!(output.hits[100].id, 100);
    }
    #[test]
    fn renamed_catalog_accounts() {
        let mut a = doc(1, "主力刷流站");
        a.catalog_id = Some("qingwa".into());
        a.result = Some("failed".into());
        let mut b = a.clone();
        b.id = 2;
        b.result = Some("success".into());
        for q in ["主力刷流站", "青蛙", "QingWa", "qingwa", "new.qingwa.pro"] {
            assert_eq!(ids(&[a.clone(), b.clone()], q), vec![1, 2], "{q}");
        }
        assert_eq!(ids(&[a, b], "青蛙 失败"), vec![1]);
        assert_eq!(catalog_id_for_ptd_id("mteam"), Some("m-team".into()));
        assert_eq!(catalog_id_for_ptd_id("m--team"), None);
    }
    #[test]
    fn prepared_batches_preserve_global_name_ambiguity_and_order() {
        let mut a = doc(3, "失败");
        a.result = Some("success".into());
        let mut b = doc(2, "备用");
        b.result = Some("failed".into());
        let docs = vec![a, b];
        let mut context = SearchContext::default();
        for batch in docs.chunks(1) {
            context_observe(&mut context, batch, "失败").unwrap();
        }
        let prepared = prepare_context("失败", &SearchFilters::default(), context).unwrap();
        let mut hits = Vec::new();
        for batch in docs.chunks(1) {
            hits.extend(search_prepared(batch, &prepared).unwrap().hits);
        }
        hits.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| b.similarity.total_cmp(&a.similarity))
                .then_with(|| a.id.cmp(&b.id))
        });
        assert_eq!(
            hits.iter().map(|h| h.id).collect::<Vec<_>>(),
            ids(&docs, "失败")
        );
        assert_eq!(hits[0].id, 3);
    }
    #[test]
    fn description_spans_keep_entity_and_state_constraints() {
        let mut a = doc(1, "青蛙");
        a.catalog_id = Some("qingwa".into());
        a.result = Some("failed".into());
        let mut b = a.clone();
        b.id = 2;
        b.result = Some("success".into());
        let mixed = search(&[a.clone(), b.clone()], "青蛙 banana", &Default::default()).unwrap();
        assert!(mixed.hits.is_empty());
        assert_eq!(mixed.semantic_status, "not_needed");
        let filtered = search(&[a, b], "动漫 result:failed", &Default::default()).unwrap();
        assert_eq!(
            filtered.hits.iter().map(|h| h.id).collect::<Vec<_>>(),
            vec![1]
        );
        let prose = search(
            &[doc(1, "private custom site")],
            "unfamiliar descriptive words",
            &Default::default(),
        )
        .unwrap();
        assert_ne!(prose.semantic_status, "not_needed");
        assert!(prose.hits.is_empty());
        let quoted = search(
            &[doc(1, "private custom site")],
            "\"unfamiliar descriptive words\"",
            &Default::default(),
        )
        .unwrap();
        assert_eq!(quoted.semantic_status, "not_needed");
        assert!(quoted.hits.is_empty());
    }
    #[test]
    fn site_field_uses_local_association_not_task_name() {
        let mut task = doc(1, "签到任务");
        task.site_names = vec!["主力刷流站".into()];
        task.site_fields = vec!["proxy.example.invalid".into()];
        task.catalog_id = Some("qingwa".into());
        for q in [
            "site:\"主力刷流站\"",
            "site:proxy.example.invalid",
            "site:青蛙",
        ] {
            assert_eq!(ids(&[task.clone()], q), vec![1], "{q}");
        }
        assert!(ids(&[task.clone()], "site:签到任务").is_empty());
        assert!(ids(&[task], "site:动漫").is_empty());
    }
    #[test]
    fn prose_grammar_ignores_sentence_punctuation_and_pronoun_aliases() {
        for word in [
            "cinema?",
            "animation.",
            "training!",
            "old-fashioned",
            "I'm",
            "children’s",
        ] {
            assert!(prose_word(&normalize(word)).is_some(), "{word}");
        }
        for literal in ["42", "abc42", "42abc", "tracker.example", "12.34"] {
            assert!(prose_word(literal).is_none(), "{literal}");
        }
        let mut terms = tokenize("find animated films for me.").unwrap();
        let clause = split_description(&mut terms, false, |t| compact(&t.value) == "me");
        assert!(terms.is_empty());
        assert_eq!(clause.join(" "), "find animated films for me.");
        let mut terms = tokenize("find animated films site:me").unwrap();
        split_description(&mut terms, false, |t| compact(&t.value) == "me");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].field.as_deref(), Some("site"));
        let mut terms = tokenize("find animated films \"me\"").unwrap();
        split_description(&mut terms, false, |t| compact(&t.value) == "me");
        assert_eq!(terms.len(), 1);
        assert!(terms[0].quoted);
        let mut terms = tokenize("me").unwrap();
        split_description(&mut terms, true, |_| true);
        assert_eq!(terms.len(), 1);
        let mut terms = tokenize("青蛙 banana").unwrap();
        split_description(&mut terms, false, |_| false);
        assert_eq!(terms.len(), 2);
    }
    #[test]
    fn exact_collections_are_hard_membership_constraints() {
        let docs: Vec<_> = catalog()
            .iter()
            .enumerate()
            .map(|(i, e)| SearchDocument {
                id: i as i64,
                names: vec![format!("local account {i}")],
                catalog_id: Some(e.id.clone()),
                ..Default::default()
            })
            .collect();
        for (query, label) in [
            ("十二大", "十二大"),
            ("十二大？", "十二大"),
            ("top 12 chinese trackers", "十二大"),
            ("traditional nine", "传统九大"),
            ("高校站", "高校站"),
        ] {
            let result = search(&docs, query, &Default::default()).unwrap();
            let mut expected: Vec<_> = catalog()
                .iter()
                .enumerate()
                .filter(|(_, e)| e.community_labels.iter().any(|s| s == label))
                .map(|(id, _)| id as i64)
                .collect();
            expected.sort();
            assert!(!expected.is_empty());
            assert_eq!(
                result.hits.iter().map(|h| h.id).collect::<Vec<_>>(),
                expected,
                "{query}"
            );
            assert!(
                result
                    .parsed_filters
                    .iter()
                    .any(|f| f.field == "collection" && f.value == label)
            );
            assert_eq!(result.semantic_status, "not_needed");
        }
        let result = search(
            &docs,
            "十二大 animated entertainment recommendations?",
            &Default::default(),
        )
        .unwrap();
        assert!(result.hits.iter().all(|h| {
            catalog()[h.id as usize]
                .community_labels
                .contains(&"十二大".into())
        }));
        let custom = doc(999, "十二大");
        let result = search(&[custom], "十二大", &Default::default()).unwrap();
        assert_eq!(result.hits[0].id, 999);
        assert!(result.parsed_filters.is_empty());
        let quoted = search(&docs, "\"十二大\"", &Default::default()).unwrap();
        assert!(quoted.parsed_filters.is_empty());
        assert_eq!(quoted.hits.len(), 12);
        let mut context = SearchContext::default();
        context_observe(&mut context, &docs, "十二大").unwrap();
        let prepared = prepare_context("十二大", &Default::default(), context).unwrap();
        let batches: Vec<_> = docs
            .chunks(17)
            .flat_map(|batch| search_prepared(batch, &prepared).unwrap().hits)
            .map(|h| h.id)
            .collect();
        assert_eq!(batches, ids(&docs, "十二大"));
    }
    #[test]
    fn description_ranking_keeps_identity_tiers_and_soft_concept_evidence() {
        let hit = |id, rank, similarity, reason: &str| SearchHit {
            id,
            rank,
            similarity,
            matched_by: vec![reason.into()],
        };
        let concept = hit(1, result_tier(0, false, true), 0.7, "concept");
        let semantic = hit(2, result_tier(0, false, true), 0.9, "semantic");
        assert!(compare_hits(&semantic, &concept).is_lt());
        for tier in [0, 1, 2, 3, 4] {
            let lexical = hit(3, result_tier(tier, true, true), 0.0, "name");
            assert_eq!(lexical.rank, tier);
            assert!(compare_hits(&lexical, &semantic).is_lt());
        }
        let exact = hit(9, result_tier(0, true, true), 0.1, "name");
        let fuzzy = hit(1, result_tier(4, true, true), 0.99, "fuzzy");
        assert!(compare_hits(&exact, &fuzzy).is_lt());
        assert!(compare_hits(&hit(1, 5, 0.8, "concept"), &hit(2, 5, 0.8, "semantic")).is_lt());
    }
    #[test]
    fn configured_concept_phrases_share_encoder_expansion_and_keep_literal_protections() {
        let asset: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/search/concepts.json")).unwrap();
        let mut count = 0;
        for concept in asset["concepts"].as_array().unwrap() {
            for phrase in concept["terms"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t.as_str())
                .filter(|t| t.split_whitespace().count() > 1)
            {
                count += 1;
                let phrase = normalize(phrase);
                let expected = search_encoder::semantic_concepts(&phrase);
                assert!(!expected.is_empty(), "{phrase}");
                assert_eq!(semantic::expansions(&phrase), expected);
                let mut terms = tokenize(&phrase).unwrap();
                merge_concept_phrases(&mut terms);
                assert_eq!(terms.len(), 1, "{phrase}");
                let desc = split_description(&mut terms, false, |_| false);
                assert!(terms.is_empty());
                assert_eq!(desc, vec![phrase.clone()]);
                let mut terms = tokenize(&format!("\"{phrase}\"")).unwrap();
                merge_concept_phrases(&mut terms);
                assert!(split_description(&mut terms, false, |_| false).is_empty());
                assert_eq!(terms.len(), 1);
                let mut terms = tokenize(&format!("name:\"{phrase}\"")).unwrap();
                merge_concept_phrases(&mut terms);
                assert!(split_description(&mut terms, false, |_| false).is_empty());
                assert_eq!(terms.len(), 1);
                if !collection_aliases().contains_key(&phrase) {
                    let mut named = doc(1, &phrase);
                    named.result = Some("failed".into());
                    let q = format!("{phrase} result:failed");
                    let mut census = SearchContext::default();
                    context_observe(&mut census, &[named.clone()], &q).unwrap();
                    let prepared = prepare_context(&q, &SearchFilters::default(), census).unwrap();
                    let result = search_prepared(&[named], &prepared).unwrap();
                    assert_eq!(result.hits.len(), 1, "{phrase}");
                    assert_eq!(result.semantic_status, "not_needed", "{phrase}");
                }
            }
        }
        assert!(count > 1);
        let mut terms = tokenize("anime backup").unwrap();
        merge_concept_phrases(&mut terms);
        split_description(&mut terms, false, |_| false);
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].value, "backup");
        let mut terms = tokenize("find dolby vision films?").unwrap();
        merge_concept_phrases(&mut terms);
        assert_eq!(
            split_description(&mut terms, false, |_| false).join(" "),
            "find dolby vision films?"
        );
        assert!(terms.is_empty());
    }
    #[test]
    fn spaced_complete_pinyin_is_one_name_not_syllable_or_matching() {
        let docs = vec![
            doc(1, "主力刷流站"),
            doc(2, "备用PT2"),
            doc(3, "主力备用站"),
        ];
        for q in ["zhuli shualiuzhan", "zhu li shua liu zhan", "zl slz"] {
            assert_eq!(ids(&docs, q), vec![1], "{q}");
            let mut census = SearchContext::default();
            context_observe(&mut census, &docs, q).unwrap();
            let prepared = prepare_context(q, &Default::default(), census).unwrap();
            assert_eq!(
                search_prepared(&docs, &prepared)
                    .unwrap()
                    .hits
                    .iter()
                    .map(|h| h.id)
                    .collect::<Vec<_>>(),
                vec![1]
            );
        }
        assert_eq!(ids(&docs, "bei yong PT2"), vec![2]);
        assert!(ids(&docs, "zhuli missing").is_empty());
        assert!(ids(&docs, "name:\"zhuli shualiuzhan\"").is_empty());
    }
    #[test]
    fn shared_text_and_public_descriptions_equal_fresh_computation() {
        let large = format!("{}末尾", "historical failure message ".repeat(40));
        for value in [
            "主力刷流站",
            "重庆音乐PT2",
            "Ｍ－Ｔｅａｍ",
            "failure 收藏",
            large.as_str(),
        ] {
            let cached = text(value);
            let fresh = build_text(value);
            assert_eq!(cached.as_ref(), &fresh);
            for query in [
                value,
                "zhulishualiuzhan",
                "zlslz",
                "chongqingyinyuept2",
                "mteam",
                "末尾",
                "unrelated",
            ] {
                let term = Term {
                    value: normalize(query),
                    field: None,
                    quoted: false,
                };
                assert_eq!(
                    matches(&cached, &term, true, false),
                    matches(&fresh, &term, true, false)
                );
            }
        }
        let cache = TEXT_CACHE.read().unwrap_or_else(|e| e.into_inner());
        assert!(cache.len() <= 4096);
        assert!(cache.keys().all(|key| key.len() <= 512));
        assert!(!cache.contains_key(&large));
        drop(cache);
        for entry in catalog() {
            assert_eq!(
                public_description(&entry.id),
                Some(expanded_public_text(entry).as_str()),
                "{}",
                entry.id
            );
        }
        assert!(public_description("missing-public-id").is_none());
    }
    #[test]
    fn strict_identity() {
        assert!(parse_catalog("{broken").is_err());
        assert!(parse_catalog("{\"sites\":42}").is_err());
        assert_eq!(
            resolve_catalog("https://qingwa.example.invalid", "auto", None),
            None
        );
        assert_eq!(
            resolve_catalog("https://whatever.invalid", "none", Some("qingwa")),
            None
        );
        assert_eq!(
            resolve_catalog("https://whatever.invalid", "manual", Some("removed-id")),
            Some("removed-id".into())
        );
        assert_eq!(
            normalize_host("https://EXAMPLE.COM.:443/path?secret=1"),
            Some("example.com".into())
        );
    }
}
