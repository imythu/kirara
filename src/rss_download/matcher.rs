//! One rule engine for saved rules, previews, and the final pre-submission check.
//! Missing evidence is distinct from a rejection and never implicitly becomes a
//! zero/false value. A known rejection suppresses needless attribute requests.

use chrono::Utc;
use regex::{Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;

use super::models::{ItemRecord, MatchEvaluation, MatchReason, RuleFilters};

const MAX_KEYWORDS: usize = 50;
const MAX_KEYWORD_CHARS: usize = 100;
const MAX_REGEX_CHARS: usize = 512;

struct CompiledFilters {
    include: Vec<String>,
    exclude: Vec<String>,
    include_regex: Option<Regex>,
    exclude_regex: Option<Regex>,
}

/// NFKC and full Unicode case folding apply equally to titles and plain words.
/// Do not use the search module's transliteration: RSS words are substrings,
/// and accents and punctuation retain their literal meaning.
pub fn normalize_title(value: &str) -> String {
    let canonical: String = value.nfkc().collect();
    icu_casemap::CaseMapper::new()
        .fold_string(&canonical)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn validate_filters(filters: &RuleFilters) -> Result<(), String> {
    compile_filters(filters).map(|_| ())
}

fn compile_filters(filters: &RuleFilters) -> Result<CompiledFilters, String> {
    if !matches!(filters.include_mode.as_str(), "all" | "any") {
        return Err("包含词模式必须为 all（全部）或 any（任意）".into());
    }
    if !matches!(filters.hr_policy.as_str(), "require_clear" | "any") {
        return Err("H&R 策略必须为 require_clear（确认无 H&R）或 any（不限制）".into());
    }
    if let (Some(minimum), Some(maximum)) = (filters.min_size_bytes, filters.max_size_bytes) {
        if minimum > maximum {
            return Err("最小大小不能大于最大大小".into());
        }
    }
    let include = normalize_keywords(&filters.include, "包含词")?;
    let exclude = normalize_keywords(&filters.exclude, "排除词")?;
    let include_regex = compile_regex(filters.include_regex.as_deref(), "包含正则")?;
    let exclude_regex = compile_regex(filters.exclude_regex.as_deref(), "排除正则")?;
    if include.is_empty()
        && exclude.is_empty()
        && include_regex.is_none()
        && exclude_regex.is_none()
        && !filters.match_all
    {
        return Err("未设置标题筛选，请显式选择「匹配全部条目」".into());
    }
    Ok(CompiledFilters {
        include,
        exclude,
        include_regex,
        exclude_regex,
    })
}

fn normalize_keywords(keywords: &[String], label: &str) -> Result<Vec<String>, String> {
    if keywords.len() > MAX_KEYWORDS {
        return Err(format!("{label}最多 {MAX_KEYWORDS} 项"));
    }
    keywords
        .iter()
        .map(|keyword| {
            if keyword.chars().count() > MAX_KEYWORD_CHARS {
                return Err(format!("每个{label}最多 {MAX_KEYWORD_CHARS} 个字符"));
            }
            let keyword = normalize_title(keyword);
            if keyword.is_empty() {
                return Err(format!("{label}不能是空白文本"));
            }
            if keyword.chars().count() > MAX_KEYWORD_CHARS {
                return Err(format!(
                    "{label}规范化后不能超过 {MAX_KEYWORD_CHARS} 个字符"
                ));
            }
            Ok(keyword)
        })
        .collect()
}

fn compile_regex(expression: Option<&str>, label: &str) -> Result<Option<Regex>, String> {
    let Some(expression) = expression.filter(|expression| !expression.trim().is_empty()) else {
        return Ok(None);
    };
    if expression.chars().count() > MAX_REGEX_CHARS {
        return Err(format!("{label}最多 {MAX_REGEX_CHARS} 个字符"));
    }
    // Regex syntax must remain intact (for example, \p{Lu} cannot be case
    // folded). Expressions run against the normalized title, with Unicode
    // case-insensitive matching. Rust regex rejects lookaround/backreferences.
    RegexBuilder::new(expression)
        .unicode(true)
        .case_insensitive(true)
        .size_limit(1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .nest_limit(64)
        .build()
        .map(Some)
        .map_err(|error| format!("{label}无效：{error}"))
}

pub fn evaluate(filters: &RuleFilters, item: &ItemRecord) -> MatchEvaluation {
    evaluate_at(filters, item, Utc::now().timestamp())
}

/// An explicit clock makes promotion boundary checks deterministic in tests and
/// lets callers use one common instant for a preview batch.
pub fn evaluate_at(filters: &RuleFilters, item: &ItemRecord, now: i64) -> MatchEvaluation {
    let compiled = match compile_filters(filters) {
        Ok(compiled) => compiled,
        Err(error) => {
            return MatchEvaluation {
                matched: false,
                needs_attributes: false,
                reasons: vec![reason(
                    "invalid_rule",
                    error,
                    Some("filters"),
                    None,
                    Some("有效规则配置".into()),
                )],
            };
        }
    };
    let title = normalize_title(&item.title);
    let mut result = MatchEvaluation::default();
    let mut rejected = false;
    let mut unknown = false;
    if !item.downloadable {
        rejected = true;
        result.reasons.push(reason(
            "missing_download_locator",
            "没有可验证的种子定位；详情页链接不能直接下载",
            Some("downloadable"),
            Some("false".into()),
            Some("种子附件或受支持站点的种子 ID".into()),
        ));
    }
    if !compiled.include.is_empty() {
        let matches = compiled
            .include
            .iter()
            .filter(|keyword| title.contains(keyword.as_str()))
            .count();
        let passed = if filters.include_mode == "all" {
            matches == compiled.include.len()
        } else {
            matches > 0
        };
        rejected |= !passed;
        let expected = format!(
            "{}满足：{}",
            if filters.include_mode == "all" {
                "全部"
            } else {
                "任意"
            },
            filters.include.join("、")
        );
        result.reasons.push(reason(
            if passed {
                "include_matched"
            } else {
                "include_not_matched"
            },
            if passed {
                "包含词条件通过"
            } else {
                "标题未满足包含词条件"
            },
            Some("title"),
            Some(item.title.clone()),
            Some(expected),
        ));
    }
    if !compiled.exclude.is_empty() {
        let hits: Vec<_> = compiled
            .exclude
            .iter()
            .enumerate()
            .filter(|(_, keyword)| title.contains(keyword.as_str()))
            .map(|(index, _)| filters.exclude[index].clone())
            .collect();
        let passed = hits.is_empty();
        rejected |= !passed;
        result.reasons.push(reason(
            if passed {
                "exclude_clear"
            } else {
                "excluded_keyword"
            },
            if passed {
                "未命中排除词"
            } else {
                "标题命中排除词"
            },
            Some("title"),
            Some(if passed {
                item.title.clone()
            } else {
                hits.join("、")
            }),
            Some(format!("不包含：{}", filters.exclude.join("、"))),
        ));
    }
    if let Some(regex) = &compiled.include_regex {
        let passed = regex.is_match(&title);
        rejected |= !passed;
        result.reasons.push(reason(
            if passed {
                "include_regex_matched"
            } else {
                "include_regex_not_matched"
            },
            if passed {
                "包含正则通过"
            } else {
                "标题未满足包含正则"
            },
            Some("title"),
            Some(item.title.clone()),
            filters.include_regex.clone(),
        ));
    }
    if let Some(regex) = &compiled.exclude_regex {
        let passed = !regex.is_match(&title);
        rejected |= !passed;
        result.reasons.push(reason(
            if passed {
                "exclude_regex_clear"
            } else {
                "excluded_regex"
            },
            if passed {
                "未命中排除正则"
            } else {
                "标题命中排除正则"
            },
            Some("title"),
            Some(item.title.clone()),
            filters.exclude_regex.clone(),
        ));
    }
    if compiled.include.is_empty()
        && compiled.exclude.is_empty()
        && compiled.include_regex.is_none()
        && compiled.exclude_regex.is_none()
    {
        result.reasons.push(reason(
            "match_all",
            "已明确允许匹配全部标题",
            Some("title"),
            Some(item.title.clone()),
            Some("不限标题".into()),
        ));
    }
    if filters.min_size_bytes.is_some() || filters.max_size_bytes.is_some() {
        let expected = size_expectation(filters);
        match item.attributes.size_bytes {
            Some(size) => {
                let passed = filters.min_size_bytes.is_none_or(|minimum| size >= minimum)
                    && filters.max_size_bytes.is_none_or(|maximum| size <= maximum);
                rejected |= !passed;
                result.reasons.push(reason(
                    if passed {
                        "size_matched"
                    } else {
                        "size_out_of_range"
                    },
                    if passed {
                        "种子大小符合范围（包含边界）"
                    } else {
                        "种子大小超出范围"
                    },
                    Some("size_bytes"),
                    Some(size.to_string()),
                    Some(expected),
                ));
            }
            None => {
                unknown = true;
                result.reasons.push(missing(
                    "size_bytes",
                    "缺少种子大小，无法判断范围",
                    expected,
                ));
            }
        }
    }
    if let Some(minimum) = filters.min_seeders {
        let expected = format!(">= {minimum}");
        match item.attributes.seeders {
            Some(seeders) => {
                let passed = seeders >= minimum;
                rejected |= !passed;
                result.reasons.push(reason(
                    if passed {
                        "seeders_matched"
                    } else {
                        "seeders_below_minimum"
                    },
                    if passed {
                        "做种数符合要求"
                    } else {
                        "做种数低于要求"
                    },
                    Some("seeders"),
                    Some(seeders.to_string()),
                    Some(expected),
                ));
            }
            None => {
                unknown = true;
                result
                    .reasons
                    .push(missing("seeders", "缺少做种数，不能视为 0", expected));
            }
        }
    }
    if filters.free_only {
        // The end instant is exclusive: a promotion ending now has expired.
        // Stale source evidence is refreshed by the service before submission;
        // this pure engine always rejects an already expired known promotion.
        if let Some(end) = item.attributes.free_until.filter(|end| *end <= now) {
            rejected = true;
            result.reasons.push(reason(
                "free_expired",
                "免费期限已结束，不能按免费资源下载",
                Some("free_until"),
                Some(end.to_string()),
                Some(format!("> {now}")),
            ));
        } else {
            match item
                .attributes
                .download_volume_factor
                .filter(|factor| factor.is_finite() && *factor >= 0.0)
            {
                Some(factor) => {
                    let passed = factor == 0.0;
                    rejected |= !passed;
                    result.reasons.push(reason(
                        if passed { "free_matched" } else { "not_free" },
                        if passed {
                            "结构化证据确认下载不计流量"
                        } else {
                            "下载计费系数不为 0，并非免费"
                        },
                        Some("download_volume_factor"),
                        Some(factor.to_string()),
                        Some("0".into()),
                    ));
                }
                None => {
                    unknown = true;
                    result.reasons.push(missing(
                        "download_volume_factor",
                        "缺少可靠免费证据；标题或描述中的 FREE 仅是线索",
                        "0（可靠的结构化证据）".into(),
                    ));
                }
            }
        }
    }
    if filters.hr_policy == "require_clear" {
        // Even contradictory persisted evidence must resolve conservatively.
        let hr = if item
            .attributes
            .minimum_ratio
            .is_some_and(|ratio| ratio.is_finite() && ratio > 0.0)
            || item
                .attributes
                .minimum_seed_time
                .is_some_and(|seconds| seconds > 0)
        {
            Some(true)
        } else {
            item.attributes.hr
        };
        match hr {
            Some(hr) => {
                rejected |= hr;
                result.reasons.push(reason(
                    if hr { "hr_required" } else { "hr_clear" },
                    if hr {
                        "资源有 H&R 要求，规则仅接受确认无 H&R 的资源"
                    } else {
                        "可靠证据确认无 H&R 要求"
                    },
                    Some("hr"),
                    Some(hr.to_string()),
                    Some("false（确认无 H&R）".into()),
                ));
            }
            None => {
                unknown = true;
                result.reasons.push(missing(
                    "hr",
                    "H&R 状态未知，需关联支持属性查询的站点或调整 H&R 策略",
                    "false（确认无 H&R）".into(),
                ));
            }
        }
    }
    result.matched = !rejected && !unknown;
    result.needs_attributes = unknown && !rejected;
    result
}

fn size_expectation(filters: &RuleFilters) -> String {
    match (filters.min_size_bytes, filters.max_size_bytes) {
        (Some(minimum), Some(maximum)) => format!("{minimum} <= bytes <= {maximum}"),
        (Some(minimum), None) => format!("bytes >= {minimum}"),
        (None, Some(maximum)) => format!("bytes <= {maximum}"),
        (None, None) => "不限".into(),
    }
}

fn missing(field: &str, message: &str, expected: String) -> MatchReason {
    reason(
        "attribute_unknown",
        message,
        Some(field),
        Some("未知".into()),
        Some(expected),
    )
}

fn reason(
    code: &str,
    message: impl Into<String>,
    field: Option<&str>,
    actual: Option<String>,
    expected: Option<String>,
) -> MatchReason {
    MatchReason {
        code: code.into(),
        message: message.into(),
        field: field.map(str::to_string),
        actual,
        expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rss_download::models::ItemAttributes;

    fn item(title: &str) -> ItemRecord {
        ItemRecord {
            id: 1,
            feed_id: 1,
            feed_name: "Test".into(),
            generation: 1,
            item_key: "guid:test".into(),
            sequence: 1,
            title: title.into(),
            detail_url: None,
            site_torrent_id: None,
            published_at: None,
            categories: vec![],
            attributes: ItemAttributes::default(),
            downloadable: true,
            content_revision: 1,
            first_seen_at: "2026-09-08T00:00:00Z".into(),
            last_seen_at: "2026-09-08T00:00:00Z".into(),
            status: "observed".into(),
            decisions: vec![],
        }
    }

    fn rule() -> RuleFilters {
        RuleFilters {
            match_all: true,
            hr_policy: "any".into(),
            ..Default::default()
        }
    }

    #[test]
    fn ordinary_words_use_nfkc_full_unicode_casefold_and_collapsed_whitespace() {
        let mut filters = rule();
        filters.include = vec![
            "STRASSE".into(),
            "ｆｆｉ".into(),
            "ΟΣ".into(),
            "WEB DL".into(),
            "É".into(),
        ];
        let result = evaluate(&filters, &item("Straße ﬃ ος ＷＥＢ\u{3000}\tDL e\u{301}"));
        assert!(result.matched);
        assert!(!result.needs_attributes);
        filters.exclude = vec!["straße".into()];
        assert!(!evaluate(&filters, &item("STRASSE ﬃ ος WEB DL É")).matched);
        assert_ne!(normalize_title("ü"), normalize_title("u"));
        assert_ne!(normalize_title("WEB-DL"), normalize_title("WEB DL"));
    }

    #[test]
    fn include_modes_exclusions_and_regex_combine_with_and() {
        let mut filters = rule();
        filters.include = vec!["Documentary".into(), "1080p".into()];
        assert!(!evaluate(&filters, &item("Documentary 720p")).matched);
        filters.include_mode = "any".into();
        assert!(evaluate(&filters, &item("Documentary 720p")).matched);
        filters.include_regex = Some(r"\b(?:1080p|2160p)\b".into());
        assert!(!evaluate(&filters, &item("Documentary 720p")).matched);
        assert!(evaluate(&filters, &item("Documentary 2160p")).matched);
        filters.exclude_regex = Some("CAM|HDTS".into());
        let denied = evaluate(&filters, &item("Documentary 2160p hDtS"));
        assert!(!denied.matched);
        assert!(
            denied
                .reasons
                .iter()
                .any(|reason| reason.code == "excluded_regex")
        );
        filters.exclude = vec!["REPACK".into()];
        assert!(!evaluate(&filters, &item("Documentary 2160p Repack")).matched);
    }

    #[test]
    fn regex_runs_on_normalized_title_without_corrupting_regex_syntax() {
        let mut filters = rule();
        filters.include_regex = Some(r"^STRASSE \p{Han}{2} 1080P$".into());
        assert!(evaluate(&filters, &item("Straße  中文　１０８０ｐ")).matched);
        filters.include_regex = Some(r"\p{Lu}+".into());
        assert!(validate_filters(&filters).is_ok());
    }

    #[test]
    fn absent_unused_fields_are_not_requested_and_hard_denials_suppress_network_work() {
        let mut filters = rule();
        assert!(evaluate(&filters, &item("anything")).matched);
        filters.min_seeders = Some(0);
        let unknown = evaluate(&filters, &item("anything"));
        assert!(!unknown.matched && unknown.needs_attributes);
        assert_eq!(
            unknown
                .reasons
                .iter()
                .filter(|reason| reason.code == "attribute_unknown")
                .count(),
            1
        );
        filters.include = vec!["required".into()];
        let denied = evaluate(&filters, &item("different"));
        assert!(!denied.matched && !denied.needs_attributes);
        let reason = denied
            .reasons
            .iter()
            .find(|reason| reason.code == "include_not_matched")
            .unwrap();
        assert_eq!(reason.actual.as_deref(), Some("different"));
        assert!(reason.expected.as_deref().unwrap().contains("required"));
        let mut unavailable = item("required");
        unavailable.downloadable = false;
        assert!(!evaluate(&filters, &unavailable).needs_attributes);
    }

    #[test]
    fn size_and_seeders_ranges_include_boundaries_and_zero_is_not_missing() {
        let mut filters = rule();
        filters.min_size_bytes = Some(1024);
        filters.max_size_bytes = Some(2048);
        filters.min_seeders = Some(0);
        let mut candidate = item("Example");
        candidate.attributes.seeders = Some(0);
        for size in [1024, 2048] {
            candidate.attributes.size_bytes = Some(size);
            assert!(evaluate(&filters, &candidate).matched);
        }
        for size in [1023, 2049] {
            candidate.attributes.size_bytes = Some(size);
            assert!(!evaluate(&filters, &candidate).matched);
        }
        candidate.attributes.size_bytes = Some(1024);
        filters.min_seeders = Some(1);
        assert!(!evaluate(&filters, &candidate).matched);
        candidate.attributes.seeders = Some(1);
        assert!(evaluate(&filters, &candidate).matched);
    }

    #[test]
    fn strict_free_and_hr_require_reliable_fields_and_expiry_is_exclusive() {
        let mut filters = rule();
        filters.free_only = true;
        filters.hr_policy = "require_clear".into();
        let mut candidate = item("[FREE] no H&R");
        candidate.attributes.hints = vec!["freeleech".into()];
        assert!(evaluate_at(&filters, &candidate, 100).needs_attributes);
        candidate.attributes.download_volume_factor = Some(0.0);
        candidate.attributes.hr = Some(false);
        candidate.attributes.free_until = Some(101);
        assert!(evaluate_at(&filters, &candidate, 100).matched);
        let expired = evaluate_at(&filters, &candidate, 101);
        assert!(!expired.matched && !expired.needs_attributes);
        assert!(
            expired
                .reasons
                .iter()
                .any(|reason| reason.code == "free_expired")
        );
        candidate.attributes.free_until = None;
        candidate.attributes.download_volume_factor = Some(f64::EPSILON / 2.0);
        assert!(!evaluate_at(&filters, &candidate, 100).matched);
        candidate.attributes.download_volume_factor = Some(0.0);
        candidate.attributes.hr = Some(true);
        assert!(!evaluate_at(&filters, &candidate, 100).matched);
        candidate.attributes.hr = Some(false);
        candidate.attributes.minimum_seed_time = Some(3600);
        assert!(!evaluate_at(&filters, &candidate, 100).matched);
        filters.hr_policy = "any".into();
        assert!(evaluate_at(&filters, &candidate, 100).matched);
    }

    #[test]
    fn invalid_or_nonfinite_promotions_are_unknown() {
        let mut filters = rule();
        filters.free_only = true;
        let mut candidate = item("Test");
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            candidate.attributes.download_volume_factor = Some(value);
            let result = evaluate(&filters, &candidate);
            assert!(!result.matched && result.needs_attributes);
        }
    }

    #[test]
    fn rule_validation_enforces_explicit_broad_matching_and_work_limits() {
        assert!(validate_filters(&RuleFilters::default()).is_err());
        assert!(validate_filters(&rule()).is_ok());
        let mut filters = rule();
        filters.include_mode = "unexpected".into();
        assert!(validate_filters(&filters).is_err());
        filters = rule();
        filters.hr_policy = "allow_unknown".into();
        assert!(validate_filters(&filters).is_err());
        filters = rule();
        filters.include = vec!["a".into(); 51];
        assert!(validate_filters(&filters).is_err());
        filters.include = vec!["字".repeat(100)];
        assert!(validate_filters(&filters).is_ok());
        filters.include = vec!["字".repeat(101)];
        assert!(validate_filters(&filters).is_err());
        filters.include = vec!["　\t ".into()];
        assert!(validate_filters(&filters).is_err());
        filters = rule();
        filters.min_size_bytes = Some(2);
        filters.max_size_bytes = Some(1);
        assert!(validate_filters(&filters).is_err());
        for expression in [r"(a)\1", "(?=a)", "[", "a{100000000}"] {
            filters = rule();
            filters.include_regex = Some(expression.into());
            assert!(
                validate_filters(&filters).is_err(),
                "accepted invalid or oversized regex"
            );
            let result = evaluate(&filters, &item("test"));
            assert!(!result.matched && !result.needs_attributes);
            assert_eq!(result.reasons[0].code, "invalid_rule");
        }
        filters = rule();
        filters.exclude_regex = Some("a".repeat(513));
        assert!(validate_filters(&filters).is_err());
    }
}
