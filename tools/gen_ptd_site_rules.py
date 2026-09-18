#!/usr/bin/env python3
"""Generate kirara site-rule fragments from PT-depiler definitions.

Reads TypeScript site definitions (local tree or GitHub raw URLs) and emits:
  1) Rust `SiteRule` entries for `src/site/rules.rs`
  2) Optional Unit3D catalog presets for `src/ptd_site_catalog.rs`

Only declarative data is extracted (selectors, labels, process paths, schema).
Custom class logic still needs hand-written adapter code.

**Output is a draft.** Review `tools/ptd_site_rules.json` / `.rs` before merging
into `src/site/rules.rs`. Full workflow: `doc/ptd-site-rules.md`.

Usage:
  python tools/gen_ptd_site_rules.py --local /path/to/PT-depiler/src/packages/site/definitions
  python tools/gen_ptd_site_rules.py --site audiences --site byrbt
  python tools/gen_ptd_site_rules.py --commit e9fae952 --site keepfrds
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path
from typing import Any

DEFAULT_COMMIT = "master"
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


def _looks_like_selector(value: str) -> bool:
    value = value.strip()
    if not value or value.startswith(")") or "contains(" in value:
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
    return bool(CSS_LIKE.match(value)) or (
        "[" in value or value.startswith(".") or value.startswith("#")
    )


def selector_strings(source: str, field: str) -> list[str]:
    """Collect CSS-like string literals from a `field: { ... selector: [...] }` block."""
    values: list[str] = []
    # Prefer explicit selector arrays near the field declaration.
    for match in re.finditer(
        rf"{re.escape(field)}\s*:\s*\{{",
        source,
    ):
        start = match.end()
        window = source[start : start + 1200]
        for array in re.findall(r"selector\s*:\s*\[(.*?)\]", window, re.S):
            for value in re.findall(r"[\"']([^\"']+)[\"']", array):
                if _looks_like_selector(value):
                    values.append(value)
        # case: { ".foo": status } maps — skip
        if values:
            break
    return list(dict.fromkeys(values))


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


def process_urls(source: str) -> list[str]:
    urls = re.findall(
        r"requestConfig\s*:\s*\{\s*url\s*:\s*[\"']([^\"']+)[\"']([^}]*)\}",
        source,
    )
    return list(dict.fromkeys(url for url, _ in urls))


def process_entries(source: str) -> list[dict[str, str]]:
    entries: list[dict[str, str]] = []
    for match in re.finditer(
        r"requestConfig\s*:\s*\{\s*url\s*:\s*[\"']([^\"']+)[\"']([^}]*)\}",
        source,
    ):
        url, rest = match.group(1), match.group(2)
        query = None
        params = re.search(r"params\s*:\s*\{([^}]*)\}", rest)
        if params:
            pairs = re.findall(
                r"[\"']?([A-Za-z0-9_]+)[\"']?\s*:\s*[\"']([^\"']+)[\"']",
                params.group(1),
            )
            if pairs:
                query = "&".join(f"{k}={v}" for k, v in pairs)
        entries.append({"url": url, "query": query or "", "raw": rest})
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


def detect_schema(source: str) -> str:
    match = re.search(r"schema\s*:\s*[\"']([^\"']+)[\"']", source)
    return match.group(1) if match else "NexusPHP"


def first_url(source: str) -> str | None:
    match = re.search(r"urls\s*:\s*\[[^\]]*[\"'](https?://[^\"']+)[\"']", source, re.S)
    return match.group(1) if match else None


def parse_definition(site_id: str, source: str) -> dict[str, Any]:
    schema = detect_schema(source)
    urls = re.findall(r"[\"'](https?://[^\"']+)[\"']", source)
    bonus_page_path = None
    bonus_page_query = None
    ajax_disabled = False
    referer = extract_ajax_referer(source)

    entries = process_entries(source)
    json_api = None
    for entry in entries:
        url = entry["url"]
        if "mprecent" in url:
            bonus_page_path = "/mprecent.php"
            bonus_page_query = "user={uid}"
        elif "mybonus" in url:
            bonus_page_path = "/mybonus.php"
            query = entry.get("query") or ""
            raw = entry.get("raw") or ""
            if "show=seed" in query or "show=seed" in raw or "show" in query and "seed" in query:
                bonus_page_query = "show=seed"
            elif re.search(r"show\s*:\s*[\"']seed[\"']", raw):
                bonus_page_query = "show=seed"
        if url.endswith("/api/userdetails.php") or url == "/api/userdetails.php":
            json_api = {
                "path": "/api/userdetails.php",
                "dialect": "keepfrds" if site_id == "keepfrds" else "generic",
            }

    if "getusertorrentlistajax" not in source and site_id == "keepfrds":
        ajax_disabled = True
    if "getusertorrentlistajax" not in source and "parseUserInfoForSeedingStatus" in source:
        if re.search(r"return flushUserInfo", source):
            ajax_disabled = True

    uploaded_sel = selector_strings(source, "uploaded")
    downloaded_sel = selector_strings(source, "downloaded")
    bonus_sel = selector_strings(source, "bonus")
    ratio_sel = selector_strings(source, "ratio")
    seeding_sel = selector_strings(source, "seeding")
    leeching_sel = selector_strings(source, "leeching")
    message_sel = selector_strings(source, "messageCount")
    bonus_hour_sel = selector_strings(source, "bonusPerHour")

    def keep_distinctive(values: list[str]) -> list[str]:
        out = []
        for value in values:
            if value.startswith("td.rowhead") or value.startswith("td.rowfollow"):
                continue
            if value.startswith("#info_block") and "myhr" in value:
                continue
            if "contains(" in value:
                continue
            out.append(value)
        return out[:6]

    return {
        "ptd_id": site_id,
        "schema": schema.lower().replace(" ", ""),
        "urls": urls[:3],
        "base_url": first_url(source),
        "bonus_labels": bonus_labels_from_source(source),
        "uploaded_selectors": keep_distinctive(uploaded_sel),
        "downloaded_selectors": keep_distinctive(downloaded_sel),
        "bonus_selectors": keep_distinctive(bonus_sel),
        "ratio_selectors": keep_distinctive(ratio_sel),
        "seeding_selectors": keep_distinctive(seeding_sel),
        "leeching_selectors": keep_distinctive(leeching_sel),
        "message_selectors": keep_distinctive(message_sel),
        "bonus_per_hour_selectors": keep_distinctive(bonus_hour_sel),
        "bonus_page_path": bonus_page_path,
        "bonus_page_query": bonus_page_query,
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
    lines = [
        "    SiteRule {",
        f'        ptd_id: "{escape_rust(meta["ptd_id"])}",',
        f"        bonus_labels: {rust_str_list(meta['bonus_labels'])},",
        f"        uploaded_selectors: {rust_str_list(meta['uploaded_selectors'])},",
        f"        downloaded_selectors: {rust_str_list(meta['downloaded_selectors'])},",
        f"        bonus_selectors: {rust_str_list(meta['bonus_selectors'])},",
        f"        ratio_selectors: {rust_str_list(meta['ratio_selectors'])},",
        f"        seeding_selectors: {rust_str_list(meta['seeding_selectors'])},",
        f"        leeching_selectors: {rust_str_list(meta['leeching_selectors'])},",
        f"        message_selectors: {rust_str_list(meta['message_selectors'])},",
        f"        bonus_per_hour_selectors: {rust_str_list(meta['bonus_per_hour_selectors'])},",
        f"        bonus_page: {bonus_page},",
        f"        user_torrent_ajax: {ajax},",
        f"        json_user_stats: {json_api},",
        "        ..SiteRule::empty("
        f'"{escape_rust(meta["ptd_id"])}")',
        "    },",
    ]
    return "\n".join(lines)


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
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--rust-out", type=Path, default=None)
    parser.add_argument("--catalog-out", type=Path, default=None)
    args = parser.parse_args()

    sites = args.site or DEFAULT_SITES
    metas: list[dict[str, Any]] = []
    for site_id in sites:
        try:
            source = fetch_definition(site_id, args.commit, args.local)
        except Exception as error:  # noqa: BLE001 - report and continue
            print(f"skip {site_id}: {error}", file=sys.stderr)
            continue
        metas.append(parse_definition(site_id, source))

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
            "// @generated by tools/gen_ptd_site_rules.py — review before merging "
            "into src/site/rules.rs\n"
        )
        args.rust_out.write_text(header + body + "\n", encoding="utf-8")
        print(f"wrote {args.rust_out}", file=sys.stderr)

    if args.catalog_out:
        body = emit_catalog_presets(metas)
        args.catalog_out.write_text(body + "\n", encoding="utf-8")
        print(f"wrote {args.catalog_out}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
