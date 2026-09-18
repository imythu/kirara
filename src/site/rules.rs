//! Declarative site parse rules distilled from PT-depiler definitions.
//!
//! Rules are data, not code: the NexusPHP adapter consults them for extra labels,
//! CSS selectors, bonus-page paths, and AJAX request context. A site that only
//! renames fields can be fixed here without rewriting the adapter.
//!
//! Workflow and review checklist: `doc/ptd-site-rules.md`.
//! Generator: `tools/gen_ptd_site_rules.py` (output must be reviewed before merge).

use scraper::{Html, Selector};

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

/// Optional JSON user-stats endpoint (KeepFRDS-style modern NexusPHP forks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonUserStatsRule {
    /// Path relative to site base, e.g. `/api/userdetails.php`.
    pub path: &'static str,
    /// Payload dialect. Only `keepfrds` is interpreted today.
    pub dialect: &'static str,
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
            bonus_page: BonusPageRule::Default,
            user_torrent_ajax: UserTorrentAjaxRule {
                disabled: false,
                headers: &[],
            },
            json_user_stats: None,
        }
    }
}

/// Built-in rules, sourced from PT-depiler definitions (MIT).
/// Keep this table small and high-value; extend via codegen later.
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
        uploaded_selectors: &["#userfas td.rowfollow", "td.rowhead:contains('传输') + td"],
        downloaded_selectors: &["#userfas td.rowfollow", "td.rowhead:contains('传输') + td"],
        ..SiteRule::empty("hdchina")
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
    SITE_RULES.iter().find(|rule| rule.ptd_id == ptd_id)
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
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            let text = element
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                return Some(text);
            }
            if let Some(title) = element.value().attr("title") {
                if !title.trim().is_empty() {
                    return Some(title.trim().to_string());
                }
            }
        }
    }
    None
}

pub fn extract_number_by_selectors(html: &str, selectors: &[&str]) -> Option<f64> {
    extract_text_by_selectors(html, selectors).and_then(|text| first_number(&text))
}

pub fn extract_size_by_selectors(html: &str, selectors: &[&str]) -> Option<u64> {
    let text = extract_text_by_selectors(html, selectors)?;
    // Bare "1.23 TiB" / "上传量 300.00 GiB" / "1,024 bytes".
    let scrubbed = text.replace(',', "");
    let expression =
        regex::Regex::new(r"(?i)([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)").ok()?;
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
        assert!(rule
            .user_torrent_ajax
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("referer")));
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
    fn keepfrds_skips_legacy_ajax_and_uses_json_api() {
        let rule = rule_for_site("keepfrds").unwrap();
        assert!(rule.user_torrent_ajax.disabled);
        assert!(rule.bonus_per_hour_selectors.contains(&"#perBonus"));
        let api = rule.json_user_stats.expect("keepfrds json api rule");
        assert_eq!(api.path, "/api/userdetails.php");
        assert_eq!(api.dialect, "keepfrds");
    }
}
