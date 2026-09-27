#!/usr/bin/env python3
"""Generate kirara site-rule fragments from PT-depiler definitions.

Reads TypeScript site definitions (local tree or GitHub raw URLs) and emits:
  1) Rust `SiteRule` entries for `src/site/rules.rs`
  2) Optional Unit3D catalog presets for `src/ptd_site_catalog.rs`

Only declarative data is extracted (selectors, labels, process paths, schema).
Custom class logic still needs hand-written adapter code.

The checked-in JSON preserves every PTD definition. Runtime rules are generated
for schemas handled by kirara adapters; custom parsing remains in those adapters.
Full workflow: `doc/ptd-site-rules.md`.

Usage:
  python tools/gen_ptd_site_rules.py --local /path/to/PT-depiler/src/packages/site/definitions
  python tools/gen_ptd_site_rules.py --site audiences --site byrbt
  python tools/gen_ptd_site_rules.py --commit e9fae952 --site keepfrds
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlsplit

DEFAULT_COMMIT = "569a2a68cabbeeb1329d0e9ef52f6232e3723f8d"
RAW_TEMPLATE = (
    "https://raw.githubusercontent.com/pt-plugins/PT-depiler/"
    "{commit}/src/packages/site/definitions/{site_id}.ts"
)

# NexusPHP defaults already baked into kirara adapters.
DEFAULT_BONUS_LABELS = {
    "魔力值",
    "Karma Points",
    "魅力值",
    "星焱",
    "沙粒",
    "魔力",
    "Bonus",
    "蝌蚪",
    "U币",
    "UBits Coin",
    "UCoin",
    "憨豆",
}
DEFAULT_UPLOADED_LABELS = {"上传量", "上傳量", "Uploaded"}
DEFAULT_DOWNLOADED_LABELS = {"下载量", "下載量", "Downloaded"}

# High-value sites to process when --site is omitted.
DEFAULT_SITES = [
    "audiences",
    "byrbt",
    "keepfrds",
    "u2",
    "ourbits",
    "hdchina",
    "hdsky",
    "chdbits",
    "blutopia",
    "aither",
    "huno",
    "fearnopeer",
    "shareisland",
    "mteam",
]


def fetch_definition(site_id: str, commit: str, local_dir: Path | None) -> str:
    if local_dir is not None:
        path = local_dir / f"{site_id}.ts"
        if not path.exists():
            raise FileNotFoundError(path)
        return path.read_text(encoding="utf-8")
    url = RAW_TEMPLATE.format(commit=commit, site_id=site_id)
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read().decode("utf-8")


CSS_LIKE = re.compile(
    r"^(?:\.|#|a\[|td\[|th\[|span\[|div|li\.|ul|dl|time|img\[|"
    r"font\[|table|h1|h2|i\.|li\[|\.site-|\.torrent-|\.ratio-|"
    r"span\.|a\.|dd|dt|#info|#msg|#user|#per|#outer|"
    r"a\.href|span:has|li\.ratio)"
)

PAIRS = {"{": "}", "[": "]", "(": ")"}


def regex_literal_end(source: str, index: int, end: int | None = None) -> int | None:
    end = len(source) if end is None else end
    if source[index] != "/" or index + 1 >= end:
        return None
    previous = index - 1
    while previous >= 0 and source[previous].isspace():
        previous -= 1
    before = source[previous] if previous >= 0 else ""
    prefix = source[max(0, previous - 8) : previous + 1]
    if before not in "([{,:;=!?&|" and not prefix.rstrip().endswith("return"):
        return None
    cursor = index + 1
    in_character_class = False
    while cursor < end:
        char = source[cursor]
        if char == "\\":
            cursor += 2
            continue
        if char == "[":
            in_character_class = True
        elif char == "]":
            in_character_class = False
        elif char == "/" and not in_character_class:
            cursor += 1
            while cursor < end and source[cursor].isalpha():
                cursor += 1
            return cursor
        elif char in "\r\n":
            return None
        cursor += 1
    return None


def matching_delimiter(source: str, start: int) -> int | None:
    """Return the matching close delimiter while ignoring strings and comments."""
    if start >= len(source) or source[start] not in PAIRS:
        return None
    stack = [source[start]]
    index = start + 1
    while index < len(source):
        char = source[index]
        if char in "\"'`":
            quote = char
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == quote:
                    index += 1
                    break
                else:
                    index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            index = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            end = source.find("*/", index + 2)
            index = len(source) if end < 0 else end + 2
            continue
        regex_end = regex_literal_end(source, index)
        if regex_end is not None:
            index = regex_end
            continue
        if char == "\\":
            index += 2
            continue
        if char in PAIRS:
            stack.append(char)
        elif char in PAIRS.values():
            if not stack or PAIRS[stack[-1]] != char:
                return None
            stack.pop()
            if not stack:
                return index
        index += 1
    return None


def skip_space_comments(source: str, index: int, end: int) -> int:
    while index < end:
        if source[index].isspace() or source[index] == ",":
            index += 1
        elif source.startswith("//", index):
            newline = source.find("\n", index + 2, end)
            index = end if newline < 0 else newline + 1
        elif source.startswith("/*", index):
            comment_end = source.find("*/", index + 2, end)
            index = end if comment_end < 0 else comment_end + 2
        else:
            break
    return index


def expression_end(source: str, start: int, end: int) -> int:
    index = start
    while index < end:
        if source[index] in "\"'`":
            quote = source[index]
            index += 1
            while index < end:
                if source[index] == "\\":
                    index += 2
                elif source[index] == quote:
                    index += 1
                    break
                else:
                    index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2, end)
            index = end if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            comment_end = source.find("*/", index + 2, end)
            index = end if comment_end < 0 else comment_end + 2
            continue
        regex_end = regex_literal_end(source, index, end)
        if regex_end is not None:
            index = regex_end
            continue
        if source[index] in PAIRS:
            close = matching_delimiter(source, index)
            if close is None or close >= end:
                return end
            index = close + 1
            continue
        if source[index] == ",":
            return index
        index += 1
    return end


def object_properties(source: str, start: int, end: int) -> dict[str, tuple[int, int]]:
    """Read direct properties from a TypeScript object literal range."""
    properties: dict[str, tuple[int, int]] = {}
    index = start + 1
    while index < end:
        index = skip_space_comments(source, index, end)
        if index >= end or source[index] == "}":
            break
        if source.startswith("...", index):
            index = expression_end(source, index + 3, end) + 1
            continue
        if source[index] in "\"'":
            quote = source[index]
            key_start = index + 1
            index += 1
            while index < end and source[index] != quote:
                index += 2 if source[index] == "\\" else 1
            key = source[key_start:index]
            index = min(index + 1, end)
        else:
            key_match = re.match(r"[A-Za-z_$][\w$]*", source[index:end])
            if not key_match:
                index = expression_end(source, index, end) + 1
                continue
            key = key_match.group(0)
            index += len(key)
        index = skip_space_comments(source, index, end)
        if index >= end or source[index] != ":":
            index = expression_end(source, index, end) + 1
            continue
        index = skip_space_comments(source, index + 1, end)
        value_start = index
        if index < end and source[index] in PAIRS:
            close = matching_delimiter(source, index)
            if close is None or close > end:
                break
            value_end = close + 1
            index = value_end
        else:
            value_end = expression_end(source, index, end)
            index = value_end
        properties[key] = (value_start, value_end)
        index += 1
    return properties


def object_body(source: str, value: tuple[int, int] | None) -> tuple[int, int] | None:
    if value is None:
        return None
    start, end = value
    if start >= end or source[start] != "{":
        return None
    return start, end - 1


def array_strings(source: str, value: tuple[int, int] | None) -> list[str]:
    if value is None:
        return []
    start, end = value
    if start >= end or source[start] != "[":
        text = source[start:end].strip()
        return [unquote_ts(text)] if text.startswith(("\"", "'")) else []
    result: list[str] = []
    index = start + 1
    while index < end - 1:
        if source[index].isspace() or source[index] == ",":
            index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2, end)
            index = end if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            comment_end = source.find("*/", index + 2, end)
            index = end if comment_end < 0 else comment_end + 2
            continue
        if source[index] in "\"'":
            quote = source[index]
            literal_start = index
            index += 1
            while index < end:
                if source[index] == "\\":
                    index += 2
                elif source[index] == quote:
                    index += 1
                    break
                else:
                    index += 1
            result.append(unquote_ts(source[literal_start:index]))
            continue
        if source[index] in PAIRS:
            close = matching_delimiter(source, index)
            index = end if close is None else close + 1
            continue
        index += 1
    return list(dict.fromkeys(result))


def unquote_ts(value: str) -> str:
    value = value.strip()
    if len(value) < 2 or value[0] not in "\"'" or value[-1] != value[0]:
        return value
    body = value[1:-1]
    return body.replace("\\'", "'").replace('\\"', '"').replace("\\\\", "\\")


def property_string(source: str, properties: dict[str, tuple[int, int]], key: str) -> str | None:
    value = properties.get(key)
    if value is None:
        return None
    start, end = value
    raw = source[start:end].strip()
    return unquote_ts(raw) if raw.startswith(("\"", "'")) else None


def property_number(source: str, properties: dict[str, tuple[int, int]], key: str) -> float | None:
    value = properties.get(key)
    if value is None:
        return None
    raw = source[value[0] : value[1]].strip()
    try:
        return float(raw)
    except ValueError:
        return None


def find_site_metadata(source: str) -> tuple[int, int] | None:
    match = re.search(r"\b(?:export\s+)?const\s+siteMetadata\b[^=]*=\s*\{", source)
    if not match:
        return None
    start = source.find("{", match.start(), match.end())
    end = matching_delimiter(source, start)
    return (start, end) if end is not None else None


def array_object_bodies(source: str, value: tuple[int, int] | None) -> list[tuple[int, int]]:
    if value is None:
        return []
    start, end = value
    if start >= end or source[start] != "[":
        return []
    items: list[tuple[int, int]] = []
    index = start + 1
    while index < end - 1:
        if source[index].isspace() or source[index] == ",":
            index += 1
        elif source.startswith("//", index):
            newline = source.find("\n", index + 2, end)
            index = end if newline < 0 else newline + 1
        elif source.startswith("/*", index):
            comment_end = source.find("*/", index + 2, end)
            index = end if comment_end < 0 else comment_end + 2
        elif source[index] in "\"'`":
            quote = source[index]
            index += 1
            while index < end:
                if source[index] == "\\":
                    index += 2
                elif source[index] == quote:
                    index += 1
                    break
                else:
                    index += 1
        elif source[index] in PAIRS:
            close = matching_delimiter(source, index)
            if close is None or close >= end:
                break
            if source[index] == "{":
                items.append((index, close))
            index = close + 1
        else:
            index += 1
    return items


def decode_field_selectors(
    source: str,
    selectors_value: tuple[int, int] | None,
    json_paths: bool = False,
) -> dict[str, dict[str, Any]]:
    body = object_body(source, selectors_value)
    if body is None:
        return {}
    fields: dict[str, dict[str, Any]] = {}
    for field, field_value in object_properties(source, *body).items():
        field_body = object_body(source, field_value)
        if field_body is None:
            continue
        properties = object_properties(source, *field_body)
        if json_paths:
            path = property_string(source, properties, "selector")
            if path:
                fields[field] = {"selectors": [], "attr": None, "json_path": path}
            continue
        selectors = array_strings(source, properties.get("selector"))
        selectors = [selector for selector in selectors if _looks_like_selector(selector)]
        if selectors:
            fields[field] = {
                "selectors": selectors[:12],
                "attr": property_string(source, properties, "attr"),
            }
    return fields


def _looks_like_selector(value: str) -> bool:
    value = value.strip()
    if not value or value.startswith(")"):
        return False
    if value.count("[") != value.count("]"):
        return False
    if value.count("(") != value.count(")"):
        return False
    if value in {"td", "tr", "div", "span", "a", "li", "dd", "dt", ":self"}:
        return False
    # Reject truncated fragments such as `a[href*=`
    if value.endswith("[") or value.endswith("=") or value.endswith(" "):
        return False
    return bool(CSS_LIKE.match(value)) or bool(
        re.search(r"(?:^|[ >+~])(?:[A-Za-z][\w-]*)?(?:[.#][\w-]+|\[[^]]+\])", value)
    ) or value in {"td", "tr", "div", "span", "a", "li", "dd", "dt"}


def bonus_labels_from_source(source: str) -> list[str]:
    labels: list[str] = []
    for match in re.finditer(
        r"createUserBonusSelectorFn\(\s*\[(.*?)\]\s*\)",
        source,
        re.S,
    ):
        labels.extend(re.findall(r"[\"']([^\"']+)[\"']", match.group(1)))
    for match in re.finditer(r"contains\([\"']([^\"']+)[\"']\)", source):
        value = match.group(1)
        if value in DEFAULT_BONUS_LABELS or len(value) <= 1:
            continue
        if any(
            token in value
            for token in (
                "魔力",
                "Bonus",
                "积分",
                "UCoin",
                "爆米花",
                "豆",
                "粒",
                "焱",
                "Karma",
            )
        ):
            labels.append(value)
    extras = [label for label in dict.fromkeys(labels) if label not in DEFAULT_BONUS_LABELS]
    return extras


def request_entries(source: str, process_value: tuple[int, int] | None) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for item_start, item_end in array_object_bodies(source, process_value):
        properties = object_properties(source, item_start, item_end)
        request_body = object_body(source, properties.get("requestConfig"))
        if request_body is None:
            continue
        request = object_properties(source, *request_body)
        url = property_string(source, request, "url")
        if not url:
            continue
        params_body = object_body(source, request.get("params"))
        params = object_properties(source, *params_body) if params_body else {}
        query_pairs: list[tuple[str, str]] = []
        for key, value in params.items():
            raw = source[value[0] : value[1]].strip()
            if raw.startswith(("\"", "'")):
                parsed = unquote_ts(raw)
            elif re.fullmatch(r"-?\d+(?:\.\d+)?", raw):
                parsed = raw
            else:
                continue
            parsed = parsed.replace("$user.id$", "{uid}").replace("$id$", "{uid}")
            query_pairs.append((key, parsed))
        assertion_body = object_body(source, properties.get("assertion"))
        assertions = object_properties(source, *assertion_body) if assertion_body else {}
        for field, path_key in assertions.items():
            target = property_string(source, assertions, field)
            if target and target.startswith("params."):
                key = target.removeprefix("params.")
                if key not in {name for name, _ in query_pairs}:
                    query_pairs.append((key, "{uid}" if field == "id" else "{" + field + "}"))
        fields = array_strings(source, properties.get("fields"))
        response_type = property_string(source, request, "responseType") or "document"
        method = property_string(source, request, "method") or "GET"
        selectors = (
            {}
            if response_type in {"json", "text"}
            else decode_field_selectors(source, properties.get("selectors"))
        )
        json_fields = (
            decode_field_selectors(source, properties.get("selectors"), json_paths=True)
            if response_type == "json"
            else {}
        )
        selectors.update(json_fields)
        entries.append(
            {
                "url": url,
                "query": urlencode(query_pairs),
                "fields": fields,
                "selectors": selectors,
                "response_type": response_type,
                "method": method.upper(),
            }
        )
    return entries


def extract_ajax_referer(source: str) -> str | None:
    match = re.search(r"Referer\s*:\s*[\"']([^\"']+)[\"']", source)
    if match:
        return match.group(1)
    match = re.search(r"rot13\([\"']([^\"']+)[\"']\)", source)
    if match:
        return rot13(match.group(1))
    return None


def rot13(value: str) -> str:
    def shift(ch: str) -> str:
        if "a" <= ch <= "z":
            return chr((ord(ch) - ord("a") + 13) % 26 + ord("a"))
        if "A" <= ch <= "Z":
            return chr((ord(ch) - ord("A") + 13) % 26 + ord("A"))
        return ch

    return "".join(shift(ch) for ch in value)


def parse_definition(site_id: str, source: str) -> dict[str, Any]:
    metadata_range = find_site_metadata(source)
    metadata = object_properties(source, *metadata_range) if metadata_range else {}
    schema = property_string(source, metadata, "schema") or "NexusPHP"
    urls = array_strings(source, metadata.get("urls"))
    user_info_range = object_body(source, metadata.get("userInfo"))
    user_info = object_properties(source, *user_info_range) if user_info_range else {}
    field_selectors = decode_field_selectors(source, user_info.get("selectors"))
    bonus_page_path = None
    bonus_page_query = None
    ajax_disabled = False
    referer = extract_ajax_referer(source[user_info_range[0] : user_info_range[1]]) if user_info_range else None
    entries = request_entries(source, user_info.get("process"))
    json_api = None
    profile_path = None
    profile_query = None
    identity_path = None
    identity_query = None
    process_selectors: dict[str, dict[str, Any]] = {}
    processes: list[dict[str, Any]] = []
    for entry in entries:
        url = entry["url"]
        split = urlsplit(url)
        path = split.path or url
        for source_token, target_token in (
            ("$user.id$", "{uid}"),
            ("$id$", "{uid}"),
            ("$user.name$", "{name}"),
            ("$name$", "{name}"),
            ("%7Buid%7D", "{uid}"),
            ("%7buid%7d", "{uid}"),
            ("%7Bname%7D", "{name}"),
            ("%7bname%7d", "{name}"),
        ):
            path = path.replace(source_token, target_token)
        query = split.query or entry.get("query") or ""
        for source_token, target_token in (
            ("$user.id$", "{uid}"),
            ("$id$", "{uid}"),
            ("$user.name$", "{name}"),
            ("$name$", "{name}"),
            ("%7Buid%7D", "{uid}"),
            ("%7buid%7d", "{uid}"),
            ("%7Bname%7D", "{name}"),
            ("%7bname%7d", "{name}"),
        ):
            query = query.replace(source_token, target_token)
        if "mprecent" in path:
            bonus_page_path = "/mprecent.php"
            bonus_page_query = "user={uid}"
        elif "mybonus" in path:
            bonus_page_path = path if path.startswith("/") else f"/{path}"
            if "show=seed" in query:
                bonus_page_query = "show=seed"
        if entry.get("response_type") == "json" and "userdetails" in path.lower():
            json_api = {
                "path": path if path.startswith("/") else f"/{path}",
                "dialect": "keepfrds" if site_id == "keepfrds" else "ptd",
            }
        field_names = list(dict.fromkeys([*entry["fields"], *entry["selectors"]]))
        process_fields = []
        for field in field_names:
            config = entry["selectors"].get(field) or field_selectors.get(field) or {
                "selectors": [],
                "attr": None,
                "json_path": None,
            }
            if entry.get("response_type") == "json" and not config.get("json_path"):
                json_path = next(iter(config.get("selectors", [])), None)
                if json_path:
                    config = {"selectors": [], "attr": None, "json_path": json_path}
            process_fields.append({"field": field, **config})
            if entry.get("response_type") == "json" and config.get("json_path"):
                field_selectors[field] = config
            elif field in entry["selectors"]:
                process_selectors[field] = config
        processes.append(
            {
                "path": path if path.startswith("/") else f"/{path}",
                "query": query,
                "method": entry["method"],
                "response_type": entry["response_type"],
                "fields": process_fields,
            }
        )
        if "id" in field_names and identity_path is None and entry.get("response_type") != "json":
            identity_path = path
            identity_query = query or None
        if set(field_names) & {
            "name", "uploaded", "downloaded", "trueUploaded", "trueDownloaded", "levelName"
        } and entry.get("response_type") != "json" and not ("bonus" in path.lower() or "mprecent" in path.lower()) and profile_path is None:
            profile_path = path
            profile_query = query or None
        process_selectors.update(entry["selectors"])

    field_selectors.update(process_selectors)
    if "getusertorrentlistajax" not in source and site_id == "keepfrds":
        ajax_disabled = True
    if "getusertorrentlistajax" not in source and "parseUserInfoForSeedingStatus" in source:
        if re.search(r"return flushUserInfo", source):
            ajax_disabled = True

    def selectors(field: str) -> list[str]:
        return field_selectors.get(field, {}).get("selectors", [])

    def labels_for(field: str) -> list[str]:
        labels: list[str] = []
        for selector in selectors(field):
            labels.extend(re.findall(r":contains\(['\"]([^'\"]+)['\"]\)", selector))
        return list(dict.fromkeys(label for label in labels if len(label) > 1))

    bonus_labels = bonus_labels_from_source(
        source[user_info_range[0] : user_info_range[1]] if user_info_range else ""
    )
    donor_config_body = object_body(source, user_info.get("donorConfig"))
    donor_config = object_properties(source, *donor_config_body) if donor_config_body else {}
    donor_bonus_multiplier = property_number(source, donor_config, "bonusPerHourMultiplier")
    urls = [
        rot13(url) if url.startswith(("uggcf://", "uggc://")) else url
        for url in urls
    ]
    urls = [url for url in urls if url.startswith(("http://", "https://"))]

    return {
        "ptd_id": site_id,
        "schema": schema.lower().replace(" ", ""),
        "urls": urls[:3],
        "base_url": next(iter(urls), None),
        "bonus_labels": bonus_labels,
        "uploaded_labels": labels_for("uploaded"),
        "downloaded_labels": labels_for("downloaded"),
        "uploaded_selectors": selectors("uploaded"),
        "downloaded_selectors": selectors("downloaded"),
        "bonus_selectors": selectors("bonus"),
        "ratio_selectors": selectors("ratio"),
        "seeding_selectors": selectors("seeding"),
        "leeching_selectors": selectors("leeching"),
        "message_selectors": selectors("messageCount"),
        "level_selectors": selectors("levelName"),
        "bonus_per_hour_selectors": selectors("bonusPerHour"),
        "user_info_fields": field_selectors,
        "user_info_processes": processes,
        "bonus_page_path": bonus_page_path,
        "bonus_page_query": bonus_page_query,
        "profile_path": profile_path,
        "profile_query": profile_query,
        "identity_path": identity_path,
        "identity_query": identity_query,
        "donor_bonus_multiplier": donor_bonus_multiplier,
        "ajax_referer": referer,
        "ajax_disabled": ajax_disabled,
        "json_user_stats": json_api,
        "process_urls": [e["url"] for e in entries],
    }


def rust_str_list(values: list[str]) -> str:
    if not values:
        return "&[]"
    inner = ", ".join(f'"{escape_rust(v)}"' for v in values)
    return f"&[{inner}]"


def escape_rust(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def emit_site_rule(meta: dict[str, Any]) -> str:
    bonus_page = "BonusPageRule::Default"
    if meta.get("bonus_page_path"):
        path = meta["bonus_page_path"]
        query = meta.get("bonus_page_query") or ""
        bonus_page = (
            f'BonusPageRule::Path {{ path: "{escape_rust(path)}", '
            f'query: "{escape_rust(query)}" }}'
        )
    ajax = (
        "UserTorrentAjaxRule { disabled: true, headers: &[] }"
        if meta.get("ajax_disabled")
        else (
            f'UserTorrentAjaxRule {{ disabled: false, headers: &[("Referer", '
            f'"{escape_rust(meta["ajax_referer"])}")] }}'
            if meta.get("ajax_referer")
            else "UserTorrentAjaxRule { disabled: false, headers: &[] }"
        )
    )
    json_api = "None"
    if meta.get("json_user_stats"):
        api = meta["json_user_stats"]
        json_api = (
            f'Some(JsonUserStatsRule {{ path: "{escape_rust(api["path"])}", '
            f'dialect: "{escape_rust(api["dialect"])}" }})'
        )
    field_rules = []
    for field, config in sorted(meta.get("user_info_fields", {}).items()):
        attr = config.get("attr")
        attr_expr = "None" if not attr else f'Some("{escape_rust(attr)}")'
        json_path = config.get("json_path")
        json_path_expr = "None" if not json_path else f'Some("{escape_rust(json_path)}")'
        field_rules.append(
            "            UserInfoFieldRule { "
            f'field: "{escape_rust(field)}", '
            f"selectors: {rust_str_list(config.get('selectors', []))}, "
            f"attr: {attr_expr}, json_path: {json_path_expr} "
            "},"
        )
    fields = "&[]" if not field_rules else "&[\n" + "\n".join(field_rules) + "\n        ]"
    process_rules = []
    for process in meta.get("user_info_processes", []):
        process_fields = []
        for config in process["fields"]:
            attr = config.get("attr")
            attr_expr = "None" if not attr else f'Some("{escape_rust(attr)}")'
            json_path = config.get("json_path")
            json_path_expr = "None" if not json_path else f'Some("{escape_rust(json_path)}")'
            process_fields.append(
                "            UserInfoFieldRule { "
                f'field: "{escape_rust(config["field"])}", '
                f"selectors: {rust_str_list(config.get('selectors', []))}, "
                f"attr: {attr_expr}, json_path: {json_path_expr} "
                "},"
            )
        process_field_expr = (
            "&[]" if not process_fields else "&[\n" + "\n".join(process_fields) + "\n        ]"
        )
        process_rules.append(
            "            UserInfoProcessRule { "
            f'path: "{escape_rust(process["path"])}", '
            f'query: "{escape_rust(process["query"])}", '
            f'method: "{escape_rust(process["method"])}", '
            f'response_type: "{escape_rust(process["response_type"])}", '
            f"fields: {process_field_expr} "
            "},"
        )
    processes = "&[]" if not process_rules else "&[\n" + "\n".join(process_rules) + "\n        ]"
    path = lambda key: (
        f'Some("{escape_rust(meta[key])}")' if meta.get(key) else "None"
    )
    lines = [
        "    SiteRule {",
        f'        ptd_id: "{escape_rust(meta["ptd_id"])}",',
        f"        bonus_labels: {rust_str_list(meta['bonus_labels'])},",
        f"        uploaded_labels: {rust_str_list(meta['uploaded_labels'])},",
        f"        downloaded_labels: {rust_str_list(meta['downloaded_labels'])},",
        f"        uploaded_selectors: {rust_str_list(meta['uploaded_selectors'])},",
        f"        downloaded_selectors: {rust_str_list(meta['downloaded_selectors'])},",
        f"        bonus_selectors: {rust_str_list(meta['bonus_selectors'])},",
        f"        ratio_selectors: {rust_str_list(meta['ratio_selectors'])},",
        f"        seeding_selectors: {rust_str_list(meta['seeding_selectors'])},",
        f"        leeching_selectors: {rust_str_list(meta['leeching_selectors'])},",
        f"        message_selectors: {rust_str_list(meta['message_selectors'])},",
        f"        level_selectors: {rust_str_list(meta['level_selectors'])},",
        f"        bonus_per_hour_selectors: {rust_str_list(meta['bonus_per_hour_selectors'])},",
        f"        profile_path: {path('profile_path')},",
        f"        profile_query: {path('profile_query')},",
        f"        identity_path: {path('identity_path')},",
        f"        identity_query: {path('identity_query')},",
        f"        donor_bonus_multiplier: {meta.get('donor_bonus_multiplier') or 2.0},",
        f"        user_info_fields: {fields},",
        f"        user_info_processes: {processes},",
        f"        bonus_page: {bonus_page},",
        f"        user_torrent_ajax: {ajax},",
        f"        json_user_stats: {json_api},",
        "        ..SiteRule::empty("
        f'"{escape_rust(meta["ptd_id"])}")',
        "    },",
    ]
    return "\n".join(lines)


def has_runtime_rule(meta: dict[str, Any]) -> bool:
    return bool(
        meta.get("schema") in {"nexusphp", "unit3d"}
        and (
            meta.get("user_info_fields")
            or meta.get("user_info_processes")
            or meta.get("bonus_page_path")
            or meta.get("ajax_disabled")
            or meta.get("ajax_referer")
            or meta.get("json_user_stats")
        )
    )


def all_definition_ids(commit: str, local_dir: Path | None) -> list[str]:
    if local_dir is not None:
        return sorted(path.stem for path in local_dir.glob("*.ts"))
    url = f"https://api.github.com/repos/pt-plugins/PT-depiler/git/trees/{commit}?recursive=1"
    request = urllib.request.Request(
        url,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "kirara-ptd-rule-generator"},
    )
    with urllib.request.urlopen(request, timeout=45) as response:
        tree = json.load(response)["tree"]
    prefix = "src/packages/site/definitions/"
    return sorted(
        path[len(prefix) : -3]
        for entry in tree
        if (path := entry["path"]).startswith(prefix) and path.endswith(".ts")
    )


def fetch_all_definitions(
    site_ids: list[str], commit: str, local_dir: Path | None
) -> list[tuple[str, str | Exception]]:
    results: dict[str, str | Exception] = {}
    with ThreadPoolExecutor(max_workers=min(12, max(1, len(site_ids)))) as pool:
        futures = {
            pool.submit(fetch_definition, site_id, commit, local_dir): site_id
            for site_id in site_ids
        }
        for future in as_completed(futures):
            site_id = futures[future]
            try:
                results[site_id] = future.result()
            except Exception as error:  # noqa: BLE001 - report and continue
                results[site_id] = error
    return [(site_id, results[site_id]) for site_id in site_ids]


def emit_catalog_presets(metas: list[dict[str, Any]]) -> str:
    chunks = []
    for meta in metas:
        if not str(meta.get("schema", "")).startswith("unit3d"):
            continue
        base = meta.get("base_url")
        if not base:
            continue
        chunks.append(
            "    PtdSitePreset {\n"
            f'        ptd_id: "{escape_rust(meta["ptd_id"])}",\n'
            f'        name: "{escape_rust(meta["ptd_id"])}",\n'
            '        site_type: "unit3d",\n'
            f'        base_url: "{escape_rust(base.rstrip("/"))}",\n'
            "        aliases: &[],\n"
            "    },"
        )
    return "\n".join(chunks)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commit", default=DEFAULT_COMMIT)
    parser.add_argument("--local", type=Path, default=None)
    parser.add_argument("--site", action="append", default=None)
    parser.add_argument("--all", action="store_true", help="process every PTD definition")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--rust-out", type=Path, default=None)
    parser.add_argument("--runtime-out", type=Path, default=None)
    parser.add_argument("--catalog-out", type=Path, default=None)
    args = parser.parse_args()

    sites = args.site or (all_definition_ids(args.commit, args.local) if args.all else DEFAULT_SITES)
    metas: list[dict[str, Any]] = []
    definitions = fetch_all_definitions(sites, args.commit, args.local) if args.all else [
        (site_id, fetch_definition(site_id, args.commit, args.local)) for site_id in sites
    ]
    failures = [(site_id, result) for site_id, result in definitions if isinstance(result, Exception)]
    if args.all and failures:
        for site_id, error in failures:
            print(f"failed {site_id}: {error}", file=sys.stderr)
        print(
            f"error: fetched {len(definitions) - len(failures)}/{len(definitions)} definitions; "
            "refusing to write an incomplete snapshot",
            file=sys.stderr,
        )
        return 1
    for site_id, result in definitions:
        if isinstance(result, Exception):
            print(f"skip {site_id}: {result}", file=sys.stderr)
            continue
        metas.append(parse_definition(site_id, result))

    payload = {
        "source_commit": args.commit,
        "count": len(metas),
        "sites": metas,
    }
    text = json.dumps(payload, ensure_ascii=False, indent=2)
    if args.json_out:
        args.json_out.write_text(text + "\n", encoding="utf-8")
    else:
        print(text)

    if args.rust_out:
        body = "\n".join(emit_site_rule(meta) for meta in metas)
        header = (
            "// @generated by tools/gen_ptd_site_rules.py; do not edit by hand.\n"
        )
        args.rust_out.write_text(header + body + "\n", encoding="utf-8")
        print(f"wrote {args.rust_out}", file=sys.stderr)

    if args.runtime_out:
        body = "\n".join(emit_site_rule(meta) for meta in metas if has_runtime_rule(meta))
        header = (
            "// @generated by tools/gen_ptd_site_rules.py; do not edit by hand.\n"
            "use super::{BonusPageRule, JsonUserStatsRule, SiteRule, UserInfoFieldRule, UserInfoProcessRule, UserTorrentAjaxRule};\n\n"
            "pub static PTD_SITE_RULES: &[SiteRule] = &[\n"
        )
        args.runtime_out.write_text(header + body + "\n];\n", encoding="utf-8")
        subprocess.run(
            ["rustfmt", "--edition", "2024", str(args.runtime_out)],
            check=True,
        )
        print(f"wrote {args.runtime_out}", file=sys.stderr)

    if args.catalog_out:
        body = emit_catalog_presets(metas)
        args.catalog_out.write_text(body + "\n", encoding="utf-8")
        print(f"wrote {args.catalog_out}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
