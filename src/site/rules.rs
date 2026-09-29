//! Declarative site parse rules distilled from PT-depiler definitions.
//!
//! Rules are data, not code: generated PTD field selectors and request steps feed
//! the NexusPHP and Unit3D adapters; local overrides remain in `SITE_RULES`.
//!
//! Workflow and review checklist: `doc/ptd-site-rules.md`.
//! Generator: `tools/gen_ptd_site_rules.py`; generated runtime data is checked in.

use scraper::{Element, Html, Selector};

use super::nexusphp::{first_number, size_from_parts};

/// How the adapter should fetch the bonus / hourly-rate page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BonusPageRule {
    /// Default NexusPHP `/mybonus.php` (or U2 special case handled separately).
    #[default]
    Default,
    /// `{base}{path}` — optional `{uid}` placeholder, optional query string.
    Path {
        path: &'static str,
        /// Append `?show=...` or similar.
        query: &'static str,
    },
    /// Read rate from a profile-page CSS selector instead of a bonus page.
    /// Reserved for rules that ship the rate only on the profile page.
    #[allow(dead_code)]
    ProfileSelector { selector: &'static str },
}

/// Optional override for `getusertorrentlistajax.php`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UserTorrentAjaxRule {
    /// Skip the AJAX call entirely (site removed the endpoint).
    pub disabled: bool,
    /// Extra request headers, typically `Referer`.
    pub headers: &'static [(&'static str, &'static str)],
}

/// Optional JSON user-stats endpoint for modern NexusPHP forks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonUserStatsRule {
    /// Path relative to site base, e.g. `/api/userdetails.php`.
    pub path: &'static str,
    /// Payload dialect used by the JSON parser.
    pub dialect: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct UserInfoFieldRule {
    pub field: &'static str,
    pub selectors: &'static [&'static str],
    pub attr: Option<&'static str>,
    pub json_path: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct UserInfoProcessRule {
    pub path: &'static str,
    pub query: &'static str,
    pub method: &'static str,
    pub response_type: &'static str,
    pub fields: &'static [UserInfoFieldRule],
}

/// Declarative overrides for one PTD site id.
#[derive(Debug, Clone, Copy)]
pub struct SiteRule {
    pub ptd_id: &'static str,
    /// Prepended to the adapter default bonus labels.
    pub bonus_labels: &'static [&'static str],
    pub uploaded_labels: &'static [&'static str],
    pub downloaded_labels: &'static [&'static str],
    /// Tried after label parsing fails (or when labels are empty for that site).
    pub uploaded_selectors: &'static [&'static str],
    pub downloaded_selectors: &'static [&'static str],
    pub bonus_selectors: &'static [&'static str],
    pub ratio_selectors: &'static [&'static str],
    pub seeding_selectors: &'static [&'static str],
    pub leeching_selectors: &'static [&'static str],
    pub message_selectors: &'static [&'static str],
    pub level_selectors: &'static [&'static str],
    /// CSS selectors on the profile page that already hold the hourly bonus rate.
    pub bonus_per_hour_selectors: &'static [&'static str],
    pub profile_path: Option<&'static str>,
    pub profile_query: Option<&'static str>,
    pub identity_path: Option<&'static str>,
    pub identity_query: Option<&'static str>,
    pub donor_bonus_multiplier: f64,
    pub user_info_fields: &'static [UserInfoFieldRule],
    pub user_info_processes: &'static [UserInfoProcessRule],
    pub bonus_page: BonusPageRule,
    pub user_torrent_ajax: UserTorrentAjaxRule,
    pub json_user_stats: Option<JsonUserStatsRule>,
}

impl SiteRule {
    const fn empty(ptd_id: &'static str) -> Self {
        Self {
            ptd_id,
            bonus_labels: &[],
            uploaded_labels: &[],
            downloaded_labels: &[],
            uploaded_selectors: &[],
            downloaded_selectors: &[],
            bonus_selectors: &[],
            ratio_selectors: &[],
            seeding_selectors: &[],
            leeching_selectors: &[],
            message_selectors: &[],
            level_selectors: &[],
            bonus_per_hour_selectors: &[],
            profile_path: None,
            profile_query: None,
            identity_path: None,
            identity_query: None,
            donor_bonus_multiplier: 2.0,
            user_info_fields: &[],
            user_info_processes: &[],
            bonus_page: BonusPageRule::Default,
            user_torrent_ajax: UserTorrentAjaxRule {
                disabled: false,
                headers: &[],
            },
            json_user_stats: None,
        }
    }
}

#[path = "ptd_rules_generated.rs"]
mod generated;

/// Hand-tuned overrides for definitions where the generated PTD data needs local fixes.
pub static SITE_RULES: &[SiteRule] = &[
    // Audiences: custom theme uses CSS metrics + "爆米花"; AJAX needs Referer.
    SiteRule {
        ptd_id: "audiences",
        bonus_labels: &["爆米花"],
        uploaded_selectors: &[".site-userbar__compact-metric--uploaded"],
        downloaded_selectors: &[".site-userbar__compact-metric--downloaded"],
        bonus_selectors: &[".site-userbar__compact-metric--bonus"],
        ratio_selectors: &[".site-userbar__compact-metric--ratio"],
        seeding_selectors: &[".site-userbar__compact-metric-inline-link--seeding"],
        leeching_selectors: &[".site-userbar__compact-metric-inline-link--leeching"],
        message_selectors: &[".site-userbar__compact-tool-badge--unread"],
        bonus_per_hour_selectors: &[".mybonus-side__rate"],
        // Prefer profile selectors first; bonus page still available as fallback.
        bonus_page: BonusPageRule::Default,
        user_torrent_ajax: UserTorrentAjaxRule {
            disabled: false,
            // Without this Referer the AJAX endpoint returns empty.
            headers: &[("Referer", "https://audiences.me/userdetails.php")],
        },
        ..SiteRule::empty("audiences")
    },
    // BYRBT: seeding-bonus hourly rate is under ?show=seed.
    SiteRule {
        ptd_id: "byrbt",
        message_selectors: &["#msg-bar a[href*='messages.php'] strong"],
        bonus_page: BonusPageRule::Path {
            path: "/mybonus.php",
            query: "show=seed",
        },
        ..SiteRule::empty("byrbt")
    },
    // KeepFRDS: #perBonus on profile; modern stats live in /api/userdetails.php.
    SiteRule {
        ptd_id: "keepfrds",
        bonus_per_hour_selectors: &["#info_block #perBonus", "#perBonus"],
        message_selectors: &["a[href*='messages.php'] b span[style*='color: red']"],
        user_torrent_ajax: UserTorrentAjaxRule {
            disabled: true,
            headers: &[],
        },
        json_user_stats: Some(JsonUserStatsRule {
            path: "/api/userdetails.php",
            dialect: "keepfrds",
        }),
        ..SiteRule::empty("keepfrds")
    },
    // U2: UCoin balance lives in span[title]; rate via /mprecent.php.
    SiteRule {
        ptd_id: "u2",
        bonus_labels: &["UCoin", "U币"],
        bonus_page: BonusPageRule::Path {
            path: "/mprecent.php",
            query: "user={uid}",
        },
        ..SiteRule::empty("u2")
    },
    // HDChina: theme-specific profile table classes for transfer totals.
    SiteRule {
        ptd_id: "hdchina",
        uploaded_selectors: &[
            "td.rowhead:contains('传输') + td",
            "td.rowhead:contains('傳送') + td",
            "td.rowhead:contains('Transfers') + td",
        ],
        downloaded_selectors: &[
            "td.rowhead:contains('传输') + td",
            "td.rowhead:contains('傳送') + td",
            "td.rowhead:contains('Transfers') + td",
        ],
        ..SiteRule::empty("hdchina")
    },
    // iloli: PTD addresses the profile by UUID; the numeric cookie uid only works as ?id=.
    SiteRule {
        ptd_id: "ilolicon",
        profile_path: Some("/userdetails.php"),
        profile_query: Some("id={uid}"),
        bonus_page: BonusPageRule::Path {
            path: "/mybonus.php",
            query: "",
        },
        ..SiteRule::empty("ilolicon")
    },
    // OurBits: primarily standard NexusPHP; keep empty extras.
    SiteRule {
        ptd_id: "ourbits",
        ..SiteRule::empty("ourbits")
    },
    // HDSky: dual-track levels; bonus often labeled uniquely on some themes.
    SiteRule {
        ptd_id: "hdsky",
        bonus_labels: &["魔力值", "Karma Points"],
        bonus_page: BonusPageRule::Default,
        ..SiteRule::empty("hdsky")
    },
    // CHDBits: keep generic NexusPHP labels; level/bonus naming varies by theme.
    SiteRule {
        ptd_id: "chdbits",
        ..SiteRule::empty("chdbits")
    },
];

pub fn rule_for_site(ptd_id: &str) -> Option<&'static SiteRule> {
    SITE_RULES
        .iter()
        .find(|rule| rule.ptd_id == ptd_id)
        .or_else(|| {
            generated::PTD_SITE_RULES
                .iter()
                .find(|rule| rule.ptd_id == ptd_id)
        })
}

pub fn generated_rule_for_site(ptd_id: &str) -> Option<&'static SiteRule> {
    generated::PTD_SITE_RULES
        .iter()
        .find(|rule| rule.ptd_id == ptd_id)
}

pub fn user_info_field(rule: Option<&SiteRule>, field: &str) -> Option<&'static UserInfoFieldRule> {
    rule?
        .user_info_fields
        .iter()
        .find(|entry| entry.field == field)
        .or_else(|| {
            generated_rule_for_site(rule?.ptd_id)?
                .user_info_fields
                .iter()
                .find(|entry| entry.field == field)
        })
}

pub fn extract_user_info_json_field<'a>(
    value: &'a serde_json::Value,
    rule: Option<&SiteRule>,
    field: &str,
) -> Option<&'a serde_json::Value> {
    let path = user_info_field(rule, field)?.json_path?;
    path.split('.')
        .try_fold(value, |current, key| current.get(key))
}

pub fn extract_user_info_json_value(
    value: &serde_json::Value,
    field_rule: &UserInfoFieldRule,
) -> Option<String> {
    let json_path = field_rule.json_path?;
    let selected = json_path
        .split('.')
        .try_fold(value, |current, key| current.get(key))?;
    match selected {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => Some(selected.to_string()),
    }
}

pub fn user_info_process_url(
    base_url: &str,
    path: &str,
    query: &str,
    uid: Option<&str>,
    username: Option<&str>,
) -> String {
    let path = substitute_user_values(path, uid, username);
    let query = substitute_user_values(query, uid, username);
    let base = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    if query.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{query}")
    }
}

fn substitute_user_values(value: &str, uid: Option<&str>, username: Option<&str>) -> String {
    value
        .replace("{uid}", &uid.map(encode_url_component).unwrap_or_default())
        .replace(
            "{name}",
            &username.map(encode_url_component).unwrap_or_default(),
        )
}

fn encode_url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
            encoded.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
        }
    }
    encoded
}

/// Merge adapter defaults with rule extras (rule labels first).
pub fn merge_labels(defaults: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    let mut out = Vec::with_capacity(defaults.len() + extra.len());
    for label in extra.iter().chain(defaults.iter()) {
        if !out.contains(label) {
            out.push(*label);
        }
    }
    out
}

/// Extract visible text from the first node matching any selector.
pub fn extract_text_by_selectors(html: &str, selectors: &[&str]) -> Option<String> {
    let document = Html::parse_document(html);
    for raw in selectors {
        for element in select_ptd(&document, raw) {
            if let Some(value) = element_text(element, None) {
                return Some(value);
            }
        }
    }
    None
}

pub fn extract_user_info_field(html: &str, rule: Option<&SiteRule>, field: &str) -> Option<String> {
    let field_rule = user_info_field(rule, field)?;
    extract_user_info_field_with_rule(html, field_rule)
}

pub fn extract_user_info_field_with_rule(
    html: &str,
    field_rule: &UserInfoFieldRule,
) -> Option<String> {
    let document = Html::parse_document(html);
    for raw in field_rule.selectors {
        for element in select_ptd(&document, raw) {
            if let Some(value) = element_text(element, field_rule.attr) {
                return Some(value);
            }
        }
    }
    None
}

pub fn user_info_field_matches(html: &str, rule: Option<&SiteRule>, field: &str) -> bool {
    user_info_field(rule, field)
        .is_some_and(|field_rule| user_info_field_matches_with_rule(html, field_rule))
}

pub fn user_info_field_matches_with_rule(html: &str, field_rule: &UserInfoFieldRule) -> bool {
    let document = Html::parse_document(html);
    field_rule
        .selectors
        .iter()
        .any(|selector| !select_ptd(&document, selector).is_empty())
}

fn element_text(element: scraper::ElementRef<'_>, attr: Option<&str>) -> Option<String> {
    if let Some(value) = attr
        .and_then(|name| element.value().attr(name))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    element
        .value()
        .attr("title")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let text = element
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (!text.is_empty()).then_some(text)
        })
}

fn select_ptd<'a>(document: &'a Html, raw: &str) -> Vec<scraper::ElementRef<'a>> {
    split_selector_groups(raw)
        .into_iter()
        .flat_map(|group| select_ptd_group(document, group))
        .collect()
}

fn split_selector_groups(raw: &str) -> Vec<&str> {
    // PTD selector lists can contain commas inside :contains(...) text.
    let mut groups = Vec::new();
    let mut start = 0;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in raw.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => parentheses = parentheses.saturating_add(1),
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets = brackets.saturating_add(1),
            ']' => brackets = brackets.saturating_sub(1),
            ',' if parentheses == 0 && brackets == 0 => {
                let group = raw[start..index].trim();
                if !group.is_empty() {
                    groups.push(group);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    let group = raw[start..].trim();
    if !group.is_empty() {
        groups.push(group);
    }
    groups
}

fn select_ptd_group<'a>(document: &'a Html, raw: &str) -> Vec<scraper::ElementRef<'a>> {
    let raw = raw.trim();
    let mut base_end = raw.len();
    let mut needles = Vec::new();
    let mut relation = "";
    if let Some(mut cursor) = raw.find(":contains(") {
        base_end = cursor;
        loop {
            let start = cursor + ":contains(".len();
            let tail = &raw[start..];
            let quote = tail.chars().next().filter(|ch| *ch == '\'' || *ch == '"');
            let (needle, close) = if let Some(quote) = quote {
                let needle_start = quote.len_utf8();
                let Some(needle_end) = tail[needle_start..]
                    .find(quote)
                    .map(|end| end + needle_start)
                else {
                    return Vec::new();
                };
                let close_start = needle_end + quote.len_utf8();
                let Some(close) = tail[close_start..].find(')').map(|end| end + close_start) else {
                    return Vec::new();
                };
                (tail[needle_start..needle_end].to_string(), close)
            } else {
                let Some(close) = tail.find(')') else {
                    return Vec::new();
                };
                (tail[..close].trim().to_string(), close)
            };
            needles.push(needle);
            cursor = start + close + 1;
            if raw[cursor..].starts_with(":contains(") {
                continue;
            }
            relation = raw[cursor..].trim();
            break;
        }
    }

    let (base_text, position) = strip_position_pseudo(raw[..base_end].trim());
    let Ok(base_selector) = Selector::parse(base_text) else {
        return Vec::new();
    };
    let mut elements = document
        .select(&base_selector)
        .filter(|element| {
            let text = element.text().collect::<String>();
            needles.iter().all(|needle| text.contains(needle))
        })
        .collect::<Vec<_>>();

    let (relation, relation_position) = strip_leading_position_pseudo(relation);
    if let Some(position) = position.or(relation_position) {
        elements = select_position(elements, position);
    }
    if relation.is_empty() {
        return elements;
    }
    let (adjacent, target) = if let Some(target) = relation.strip_prefix("+ ") {
        (true, target.trim())
    } else if let Some(target) = relation.strip_prefix("> ") {
        (false, target.trim())
    } else {
        return Vec::new();
    };
    let (target, target_position) = strip_position_pseudo(target);
    let Ok(target_selector) = Selector::parse(target) else {
        return Vec::new();
    };
    let mut related = elements
        .into_iter()
        .filter_map(|element| {
            if adjacent {
                let sibling = element.next_sibling_element()?;
                if target_selector.matches(&sibling) {
                    Some(sibling)
                } else {
                    sibling.select(&target_selector).next()
                }
            } else {
                element.select(&target_selector).next()
            }
        })
        .collect::<Vec<_>>();
    if let Some(position) = target_position {
        related = select_position(related, position);
    }
    related
}

#[derive(Clone, Copy)]
enum PositionPseudo {
    First,
    Last,
    Index(usize),
}

fn strip_position_pseudo(selector: &str) -> (&str, Option<PositionPseudo>) {
    if let Some(prefix) = selector.strip_suffix(":first") {
        return (prefix.trim_end(), Some(PositionPseudo::First));
    }
    if let Some(prefix) = selector.strip_suffix(":last") {
        return (prefix.trim_end(), Some(PositionPseudo::Last));
    }
    if let Some(start) = selector.rfind(":eq(")
        && let Some(index) = selector[start + 4..]
            .strip_suffix(')')
            .and_then(|value| value.parse::<usize>().ok())
    {
        return (&selector[..start], Some(PositionPseudo::Index(index)));
    }
    (selector, None)
}

fn strip_leading_position_pseudo(selector: &str) -> (&str, Option<PositionPseudo>) {
    for (pseudo, position) in [
        (":first", PositionPseudo::First),
        (":last", PositionPseudo::Last),
    ] {
        if let Some(rest) = selector.strip_prefix(pseudo) {
            return (rest.trim_start(), Some(position));
        }
    }
    if let Some(rest) = selector.strip_prefix(":eq(")
        && let Some((index, rest)) = rest.split_once(')')
        && let Ok(index) = index.parse::<usize>()
    {
        return (rest.trim_start(), Some(PositionPseudo::Index(index)));
    }
    (selector, None)
}

fn select_position<'a>(
    elements: Vec<scraper::ElementRef<'a>>,
    position: PositionPseudo,
) -> Vec<scraper::ElementRef<'a>> {
    let selected = match position {
        PositionPseudo::First => elements.first(),
        PositionPseudo::Last => elements.last(),
        PositionPseudo::Index(index) => elements.get(index),
    };
    selected.copied().into_iter().collect()
}

pub fn extract_number_by_selectors(html: &str, selectors: &[&str]) -> Option<f64> {
    extract_text_by_selectors(html, selectors).and_then(|text| first_number(&text))
}

pub fn extract_size_by_selectors(html: &str, selectors: &[&str]) -> Option<u64> {
    let text = extract_text_by_selectors(html, selectors)?;
    // Bare "1.23 TiB" / "上传量 300.00 GiB" / "1,024 bytes".
    let scrubbed = text.replace(',', "");
    let expression = regex::Regex::new(r"(?i)([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)").ok()?;
    let captures = expression.captures(&scrubbed)?;
    size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
}

pub fn extract_u64_by_selectors(html: &str, selectors: &[&str]) -> Option<u64> {
    extract_number_by_selectors(html, selectors)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value as u64)
}

pub fn extract_u32_by_selectors(html: &str, selectors: &[&str]) -> Option<u32> {
    extract_u64_by_selectors(html, selectors).and_then(|value| u32::try_from(value).ok())
}

/// Build the bonus-page URL for a site under `base_url`.
pub fn bonus_page_url(rule: Option<&SiteRule>, base_url: &str, uid: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    match rule.map(|rule| rule.bonus_page).unwrap_or_default() {
        BonusPageRule::Default => format!("{base}/mybonus.php"),
        BonusPageRule::Path { path, query } => {
            let path = path.trim_start_matches('/');
            if query.is_empty() {
                format!("{base}/{path}")
            } else if query.contains("{uid}") {
                let uid = uid.unwrap_or_default();
                format!("{base}/{path}?{}", query.replace("{uid}", uid))
            } else {
                format!("{base}/{path}?{query}")
            }
        }
        BonusPageRule::ProfileSelector { .. } => format!("{base}/mybonus.php"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_audiences_rule_and_merges_bonus_labels() {
        let rule = rule_for_site("audiences").expect("audiences rule");
        let labels = merge_labels(&["魔力值", "Karma Points"], rule.bonus_labels);
        assert_eq!(labels[0], "爆米花");
        assert!(labels.contains(&"魔力值"));
        assert!(!rule.user_torrent_ajax.disabled);
        assert!(
            rule.user_torrent_ajax
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("referer"))
        );
    }

    #[test]
    fn byrbt_bonus_page_includes_show_seed() {
        let rule = rule_for_site("byrbt").unwrap();
        assert_eq!(
            bonus_page_url(Some(rule), "https://bt.byr.cn/", Some("1")),
            "https://bt.byr.cn/mybonus.php?show=seed"
        );
    }

    #[test]
    fn u2_bonus_page_replaces_uid_placeholder() {
        let rule = rule_for_site("u2").unwrap();
        assert_eq!(
            bonus_page_url(Some(rule), "https://u2.dmhy.org", Some("42")),
            "https://u2.dmhy.org/mprecent.php?user=42"
        );
    }

    #[test]
    fn default_bonus_page_is_mybonus() {
        assert_eq!(
            bonus_page_url(None, "https://tracker.example/", None),
            "https://tracker.example/mybonus.php"
        );
    }

    #[test]
    fn extracts_metrics_from_class_only_markup() {
        let html = r#"
            <div class="site-userbar__compact-metric--uploaded">上传量 1.50 TiB</div>
            <div class="site-userbar__compact-metric--downloaded">下载量 300.00 GiB</div>
            <div class="site-userbar__compact-metric--bonus">爆米花 12,345.6</div>
            <div class="site-userbar__compact-metric--ratio">2.50</div>
        "#;
        let rule = rule_for_site("audiences").unwrap();
        assert_eq!(
            extract_size_by_selectors(html, rule.uploaded_selectors),
            Some(1649267441664)
        );
        assert_eq!(
            extract_size_by_selectors(html, rule.downloaded_selectors),
            Some(322122547200)
        );
        assert_eq!(
            extract_number_by_selectors(html, rule.bonus_selectors),
            Some(12345.6)
        );
        assert_eq!(
            extract_number_by_selectors(html, rule.ratio_selectors),
            Some(2.5)
        );
    }

    #[test]
    fn supports_ptd_last_match_followed_by_adjacent_sibling() {
        let html = r#"
            <dl>
                <dt>Invites sent</dt><dd>4</dd>
                <dt>Invites</dt><dd>7</dd>
            </dl>
        "#;
        assert_eq!(
            extract_text_by_selectors(html, &["dt:contains('Invites'):last + dd"]).as_deref(),
            Some("7")
        );
    }

    #[test]
    fn keepfrds_skips_legacy_ajax_and_uses_json_api() {
        let rule = rule_for_site("keepfrds").unwrap();
        assert!(rule.user_torrent_ajax.disabled);
        assert!(rule.bonus_per_hour_selectors.contains(&"#perBonus"));
        let api = rule.json_user_stats.expect("keepfrds json api rule");
        assert_eq!(api.path, "/api/userdetails.php");
        assert_eq!(api.dialect, "keepfrds");
    }
}
