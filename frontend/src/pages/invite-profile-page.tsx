import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  CheckCircle2,
  Copy,
  Eye,
  Gift,
  ListChecks,
  Loader2,
  Search,
  ShieldCheck,
  XCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import type { PtdSitePreset, SiteRecord } from "@/types";

type ParsedItem = {
  site_id: number;
  uid: string;
  site_label: string;
  raw: string;
  host?: string;
  error?: string;
};

type UnmatchedLink = {
  host: string;
  uid: string;
  url: string;
  reason: string;
  /** 可读站名（俗称或官方名），用于展示「本机没有」。 */
  display_name?: string;
};

type InviteParsePayload = {
  preregister_username: string | null;
  preregister_email: string | null;
  items: ParsedItem[];
  unmatched: UnmatchedLink[];
};

type LookupResult = {
  site_id: number;
  site_name: string;
  site_type: string;
  uid: string;
  username: string | null;
  email: string | null;
  uploaded: number | null;
  downloaded: number | null;
  ratio: number | null;
  join_time: number | null;
  seeding_count: number | null;
  seeding_size: number | null;
  failure: string | null;
  message: string;
};

type RowState = {
  status: "pending" | "running" | "ok" | "failed";
  result?: LookupResult;
  error?: string;
};

const COPY_HEADER = "【云母】求药信息";
const LINE_COMMENT = "# 站点名|UID";

/** 常见 PT 中文站名 → 域名片段，用于「观众https://…」这类写法。 */
const SITE_ALIAS_HINTS: Array<{ name: string; hosts: string[]; aliases?: string[] }> = [
  { name: "馒头", hosts: ["m-team.cc", "mteam.cc"], aliases: ["M-Team", "mteam"] },
  { name: "观众", hosts: ["audiences.me"], aliases: ["人人人", "Audiences"] },
  { name: "人人", hosts: ["audiences.me"], aliases: ["人人人"] },
  { name: "肉丝", hosts: ["rousi.zip"], aliases: ["RouSi"] },
  { name: "蟹黄堡", hosts: ["crabpt.vip"], aliases: ["CrabPT"] },
  { name: "青蛙", hosts: ["qingwapt.com", "qingwa.pro"], aliases: ["QingWa"] },
  { name: "家园", hosts: ["hdhome.org", "pt.keepfrds.com", "keepfrds.com"], aliases: ["HDHome", "FRDS", "朋友"] },
  { name: "fd", hosts: ["pt.keepfrds.com", "keepfrds.com"], aliases: ["FRDS"] },
  { name: "ttg", hosts: ["totheglory.im"] },
  { name: "学校", hosts: ["pt.btschool.club"], aliases: ["BTSchool"] },
  { name: "btschool", hosts: ["pt.btschool.club"] },
  { name: "末日", hosts: ["zmpt.cc"], aliases: ["末日种子库"] },
  { name: "u2", hosts: ["u2.dmhy.org"] },
  { name: "猫", hosts: ["pterclub.net", "pterclub.com"], aliases: ["猫站", "PTer", "PTerClub"] },
  { name: "猫站", hosts: ["pterclub.net", "pterclub.com"], aliases: ["PTer", "PTerClub"] },
  { name: "红豆饭", hosts: ["hdfans.org"], aliases: ["HDFans"] },
  { name: "hdfans", hosts: ["hdfans.org"], aliases: ["红豆饭"] },
  { name: "麒麟", hosts: ["hdkyl.in"], aliases: ["HDKylin"] },
  { name: "天空", hosts: ["hdsky.me"], aliases: ["HDsky", "HDS"] },
  { name: "瓷器", hosts: ["hdchina.org"], aliases: ["HDChina"] },
  { name: "白兔", hosts: [], aliases: ["OpenCD"] },
];

function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || Number.isNaN(bytes)) return "—";
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.min(sizes.length - 1, Math.floor(Math.log(Math.abs(bytes)) / Math.log(k)));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

function formatJoinTime(value: number | null | undefined): string {
  if (value == null) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleDateString();
}

function siteHost(site: SiteRecord): string {
  try {
    return new URL(site.base_url).host.toLowerCase();
  } catch {
    return site.name.toLowerCase();
  }
}

function normalizeEmail(value: string): string {
  return value.trim().toLowerCase().replace(/^mailto:/, "");
}

function stripWww(host: string): string {
  return host.replace(/^www\./i, "").toLowerCase();
}

/** 取可注册主域近似值（至少两段）。 */
function registrableDomain(host: string): string {
  const h = stripWww(host);
  const parts = h.split(".").filter(Boolean);
  if (parts.length <= 2) return h;
  return parts.slice(-2).join(".");
}

function hostsMatch(a: string, b: string): boolean {
  if (!a || !b) return false;
  const ha = stripWww(a);
  const hb = stripWww(b);
  if (ha === hb) return true;
  if (ha.endsWith(`.${hb}`) || hb.endsWith(`.${ha}`)) return true;
  return registrableDomain(ha) === registrableDomain(hb);
}

function siteNameHints(site: SiteRecord): string[] {
  const hints = new Set<string>([site.name.toLowerCase()]);
  const host = siteHost(site);
  if (host) {
    hints.add(stripWww(host));
    hints.add(registrableDomain(host));
  }
  for (const alias of SITE_ALIAS_HINTS) {
    if (alias.hosts.some((h) => hostsMatch(host, h) || hostsMatch(site.name, h))) {
      hints.add(alias.name);
      for (const h of alias.hosts) hints.add(stripWww(h));
    }
    if (site.name.includes(alias.name) || alias.name.includes(site.name)) {
      hints.add(alias.name);
      for (const h of alias.hosts) hints.add(stripWww(h));
    }
  }
  return [...hints].filter(Boolean);
}

function findSiteForHost(host: string, sites: SiteRecord[]): SiteRecord | null {
  const target = stripWww(host);
  const byHost = sites.find((site) => hostsMatch(siteHost(site), target));
  if (byHost) return byHost;

  // 中文别名 → 域名片段
  const aliasHosts = SITE_ALIAS_HINTS.filter(
    (alias) => alias.name === target || target.includes(alias.name),
  ).flatMap((alias) => alias.hosts);
  if (aliasHosts.length) {
    const byAlias = sites.find((site) =>
      aliasHosts.some((h) => hostsMatch(siteHost(site), h)),
    );
    if (byAlias) return byAlias;
  }

  const byName = sites.find((site) => {
    const name = site.name.toLowerCase();
    if (!name) return false;
    return name === target || target.includes(name) || name.includes(target);
  });
  return byName ?? null;
}

function findSiteByHint(hint: string, sites: SiteRecord[]): SiteRecord | null {
  const h = hint.trim().toLowerCase();
  if (!h) return null;
  return sites.find((site) =>
    siteNameHints(site).some((x) => x === h || x.includes(h) || h.includes(x)),
  ) ?? null;
}

/** 「人人：33317」「猫：27701」「红豆饭 64282」等圈内俗称 + UID。 */
function extractNicknameUidLines(
  text: string,
): Array<{ nickname: string; uid: string; raw: string }> {
  const out: Array<{ nickname: string; uid: string; raw: string }> = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim().replace(/^[0-9]+[.、)]\s*/, "");
    if (!line || line.startsWith("#")) continue;
    // 跳过链接行、预注册行、管道格式
    if (/https?:\/\//i.test(line) || line.includes("|") || /预注册|注册邮箱|邮箱|email|承诺|低隐私|低隐链接/.test(line)) {
      continue;
    }
    // 名称：UID  或  名称 UID
    const m =
      line.match(/^([^\s:：]{1,12})\s*[:：]\s*(\d{1,12})\s*$/) ||
      line.match(/^([^\s:：]{1,12})\s+(\d{5,12})\s*$/);
    if (!m) continue;
    const nickname = m[1].trim();
    const uid = m[2].trim();
    // 避免把「预注册ID：y13141234」误伤（非纯数字已排除）
    if (!/^\d+$/.test(uid)) continue;
    out.push({ nickname, uid, raw: line });
  }
  return out;
}

type CatalogPreset = PtdSitePreset;

function presetHost(preset: CatalogPreset): string {
  try {
    return new URL(preset.base_url).host.toLowerCase();
  } catch {
    return "";
  }
}

/** 俗称 → 已配置站点。 */
function findSiteForNickname(
  nickname: string,
  sites: SiteRecord[],
  catalog: CatalogPreset[],
): { site: SiteRecord | null; display: string } {
  const nick = nickname.trim().toLowerCase();
  const display = nickname.trim();
  if (!nick) return { site: null, display };

  const hitSite = sites.find((site) => site.name.toLowerCase() === nick);
  if (hitSite) return { site: hitSite, display: hitSite.name };

  const preset = catalog.find((item) => {
    const names = [item.name, ...(item.aliases || [])].map((x) => x.toLowerCase());
    return names.some(
      (n) => n === nick || n.includes(nick) || nick.includes(n),
    );
  });
  if (preset) {
    const host = presetHost(preset);
    const site =
      sites.find((site) => (host && hostsMatch(siteHost(site), host)) || site.name === preset.name) ??
      sites.find((site) => {
        const aliases = (preset.aliases || []).map((a) => a.toLowerCase());
        return aliases.some(
          (a) => site.name.toLowerCase() === a || site.name.toLowerCase().includes(a),
        );
      });
    return { site: site ?? null, display: `${preset.name}${(preset.aliases || []).length ? `（${preset.aliases.join("/")}）` : ""}` };
  }

  const hinted = findSiteByHint(nickname, sites);
  if (hinted) return { site: hinted, display: hinted.name };

  const alias = SITE_ALIAS_HINTS.find(
    (a) => a.name.toLowerCase() === nick || a.aliases?.some((x) => x.toLowerCase() === nick),
  );
  if (alias) {
    for (const host of alias.hosts) {
      const site = sites.find((s) => hostsMatch(siteHost(s), host));
      if (site) return { site, display: site.name };
    }
    return { site: null, display: alias.name };
  }

  return { site: null, display };
}

function extractPreregister(text: string): {
  username: string | null;
  email: string | null;
} {
  const normalized = text
    .replace(/\r/g, "")
    .replace(/[【】\[\]（）()]/g, " ")
    .replace(/[：]/g, ":");

  const usernamePatterns = [
    /预注册\s*(?:id|ID|Id|用户名|用户|账号|帐号)\s*:\s*([^\s，,。;；:]+)/,
    /注册\s*(?:id|ID|用户名|账号|帐号)\s*:\s*([^\s，,。;；:]+)/,
    /(?:申请|求药)\s*(?:id|ID|用户名)\s*:\s*([^\s，,。;；:]+)/,
  ];
  const emailPatterns = [
    /预注册\s*(?:邮箱|email|Email|E-mail|e-mail)\s*:\s*([^\s，,。;；:]+)/,
    /(?:注册|联络|联系|常用|备用)?\s*邮箱\s*:\s*([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})/,
    /(?:email|Email|E-mail)\s*:\s*([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})/,
  ];

  let username: string | null = null;
  let email: string | null = null;
  for (const re of usernamePatterns) {
    const m = normalized.match(re);
    if (m?.[1]) {
      const value = m[1].trim().replace(/^[.、,，]+|[.、,，]+$/g, "");
      if (value && !value.includes("@") && value.length <= 64) {
        username = value;
        break;
      }
    }
  }
  for (const re of emailPatterns) {
    const m = normalized.match(re);
    if (m?.[1] && m[1].includes("@")) {
      email = normalizeEmail(m[1]);
      break;
    }
  }
  if (!email) {
    // 兜底：整段里第一个看起来像邮箱的 token
    const any = text.match(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/);
    if (any?.[0]) email = normalizeEmail(any[0]);
  }
  return { username, email };
}

type RawLink = {
  url: string;
  host: string;
  uid: string;
  lineLabel: string;
  index: number;
};

function tryParseProfileUrl(rawUrl: string): { url: string; host: string; uid: string } | null {
  let candidate = rawUrl.trim();
  // 去掉尾部中英文标点与 markdown 残留
  candidate = candidate.replace(/[)\]}>」』】。，、；;！!？?,，]+$/u, "");
  // 无协议补 https
  if (/^(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)+(?:\/|$)/i.test(candidate) && !/^https?:\/\//i.test(candidate)) {
    candidate = `https://${candidate}`;
  }
  // 全角转半角（常见于粘贴）
  candidate = candidate.replace(/[：]/g, ":").replace(/[？]/g, "?").replace(/[＝]/g, "=");

  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    return null;
  }
  if (!/^https?:$/.test(parsed.protocol)) return null;

  const host = parsed.hostname.toLowerCase();
  const path = parsed.pathname.toLowerCase();
  const search = parsed.search;
  let uid = "";

  if (path.includes("userdetails.php") || path.endsWith("/userdetails.php")) {
    uid = new URLSearchParams(search).get("id") ?? "";
  } else if (path.includes("profile/detail")) {
    const m = path.match(/profile\/detail\/(\d+)/);
    uid = m?.[1] ?? "";
  } else if (path.includes("user.php")) {
    uid = new URLSearchParams(search).get("id") ?? "";
  } else if (/\/user\/(\d+)\/?$/.test(path)) {
    uid = path.match(/\/user\/(\d+)\/?$/)?.[1] ?? "";
  }

  uid = uid.trim();
  if (!uid || !/^\d+$/.test(uid)) return null;
  return { url: parsed.toString(), host, uid };
}

function extractProfileLinks(text: string): RawLink[] {
  const out: RawLink[] = [];
  const seen = new Set<string>();
  const lines = text.split(/\r?\n/);
  let offset = 0;

  for (const line of lines) {
    const lineStart = offset;
    offset += line.length + 1;

    // 行内可能有中文站名前缀：观众https://…
    const candidates = new Set<string>();
    // 完整 URL
    for (const m of line.matchAll(/https?:\/\/[^\s<>"'`]+/gi)) candidates.add(m[0]);
    // 无协议域名+路径
    for (const m of line.matchAll(/(?:^|[\s【】\[\]（）(])(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)+\/[^\s<>"'`]*/gi)) {
      candidates.add(m[0].replace(/^[\s【】\[\]（）(]+/, ""));
    }
    // Markdown [text](url)
    for (const m of line.matchAll(/\[[^\]]*\]\((https?:\/\/[^)\s]+)\)/gi)) candidates.add(m[1]);

    const lineLabel = (line.match(/^([^\s:：]{1,12})\s*https?:\/\//i)?.[1] ?? "").trim();

    for (const candidate of candidates) {
      const parsed = tryParseProfileUrl(candidate);
      if (!parsed) continue;
      const key = `${parsed.host}|${parsed.uid}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({ ...parsed, lineLabel, index: lineStart });
    }
  }
  return out;
}

function parsePipeLines(text: string): ParsedItem[] {
  const items: ParsedItem[] = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim().replace(/^[0-9]+[.、)]\s*/, "");
    if (!line || line.startsWith("#") || line === COPY_HEADER) continue;
    // 合计行
    if (line.startsWith("合计")) continue;
    if (!line.includes("|") && !/^\d+\s*[,，\s]\s*\d+$/.test(line) && !line.includes("：") && !line.includes(":")) {
      continue;
    }

    // 站点名|UID  /  站点名|站点ID|UID  /  站点名：UID
    if (line.includes("：") || (line.includes(":") && !line.includes("|"))) {
      const m = line.match(/^([^:：]{1,32})[:：]\s*(\d{1,12})\s*$/);
      if (m) {
        items.push({
          site_id: 0,
          uid: m[2],
          site_label: m[1].trim(),
          raw: line,
        });
        continue;
      }
    }

    const parts = line.includes("|")
      ? line.split("|").map((part) => part.trim())
      : line.split(/[,，\s]+/).map((part) => part.trim());

    if (parts.length >= 3) {
      const siteLabel = parts[0];
      const siteId = parts[1];
      const uid = parts[2];
      const idNum = Number(siteId);
      if (!Number.isInteger(idNum) || idNum <= 0) {
        items.push({ site_id: 0, uid, site_label: siteLabel, raw: line, error: "站点ID无效" });
        continue;
      }
      if (!uid || !/^\d+$/.test(uid)) {
        items.push({ site_id: idNum, uid, site_label: siteLabel, raw: line, error: "UID 无效" });
        continue;
      }
      items.push({ site_id: idNum, uid, site_label: siteLabel, raw: line });
      continue;
    }

    if (parts.length === 2) {
      const [left, right] = parts;
      // 站点名|UID（推荐）
      if (/^\d+$/.test(right) && !/^\d+$/.test(left)) {
        items.push({ site_id: 0, uid: right, site_label: left, raw: line });
        continue;
      }
      // 站点ID|UID
      const idNum = Number(left);
      if (Number.isInteger(idNum) && idNum > 0 && /^\d+$/.test(right)) {
        items.push({ site_id: idNum, uid: right, site_label: "", raw: line });
        continue;
      }
    }
  }
  return items;
}

/** 兼容云母管道格式与社区自由文本求药帖（预注册 + 资料链接 + 圈内俗称）。 */
export function parseInvitePayload(
  text: string,
  sites: SiteRecord[],
  catalog: CatalogPreset[] = [],
): InviteParsePayload {
  const preregister = extractPreregister(text);
  const items: ParsedItem[] = [];
  const unmatched: UnmatchedLink[] = [];
  const seenSiteUid = new Set<string>();

  const pushItem = (item: ParsedItem) => {
    const key = `${item.site_id}|${item.uid}`;
    if (seenSiteUid.has(key)) return;
    seenSiteUid.add(key);
    items.push(item);
  };

  const pushUnmatched = (entry: UnmatchedLink) => {
    const key = `${entry.host}|${entry.uid}`;
    if (unmatched.some((u) => `${u.host}|${u.uid}` === key)) return;
    unmatched.push(entry);
  };

  for (const item of parsePipeLines(text)) {
    if (item.site_id > 0) {
      if (!item.site_label) {
        const known = sites.find((site) => site.id === item.site_id);
        if (known) item.site_label = known.name;
      }
      pushItem(item);
      continue;
    }
    // 站点名|UID：按名称匹配已配置站点
    if (item.site_label && !item.error) {
      const site =
        sites.find((s) => s.name === item.site_label) ??
        sites.find((s) => s.name.toLowerCase() === item.site_label.toLowerCase());
      if (site) {
        pushItem({ ...item, site_id: site.id, site_label: site.name });
        continue;
      }
      const { site: nickSite, display } = findSiteForNickname(item.site_label, sites, catalog);
      if (nickSite) {
        pushItem({ ...item, site_id: nickSite.id, site_label: nickSite.name });
        continue;
      }
      pushUnmatched({
        host: item.site_label,
        uid: item.uid,
        url: item.raw,
        reason: "本机没有该站点",
        display_name: display || item.site_label,
      });
      continue;
    }
    pushItem(item);
  }

  for (const link of extractProfileLinks(text)) {
    let site = findSiteForHost(link.host, sites);
    if (!site && link.lineLabel) {
      site = findSiteByHint(link.lineLabel, sites);
    }
    if (!site) {
      const alias = SITE_ALIAS_HINTS.find((a) =>
        a.hosts.some((h) => hostsMatch(link.host, h)),
      );
      if (alias) site = findSiteByHint(alias.name, sites);
    }
    if (!site) {
      const preset = catalog.find((item) => {
        const host = presetHost(item);
        return host && hostsMatch(link.host, host);
      });
      if (preset) {
        site = sites.find((s) => s.name === preset.name) ?? null;
      }
    }

    if (!site) {
      const preset = catalog.find((item) => {
        const host = presetHost(item);
        return host && hostsMatch(link.host, host);
      });
      const alias = SITE_ALIAS_HINTS.find((a) =>
        a.hosts.some((h) => hostsMatch(link.host, h)),
      );
      pushUnmatched({
        host: link.host,
        uid: link.uid,
        url: link.url,
        reason: "本机没有该站点",
        display_name: preset?.name || alias?.name || link.host,
      });
      continue;
    }
    pushItem({
      site_id: site.id,
      uid: link.uid,
      site_label: site.name,
      host: link.host,
      raw: link.url,
    });
  }

  for (const row of extractNicknameUidLines(text)) {
    const { site, display } = findSiteForNickname(row.nickname, sites, catalog);
    if (!site) {
      pushUnmatched({
        host: row.nickname,
        uid: row.uid,
        url: row.raw,
        reason: "本机没有该站点",
        display_name: display || row.nickname,
      });
      continue;
    }
    pushItem({
      site_id: site.id,
      uid: row.uid,
      site_label: site.name,
      raw: row.raw,
    });
  }

  return {
    preregister_username: preregister.username,
    preregister_email: preregister.email,
    items,
    unmatched,
  };
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  document.body.removeChild(textarea);
}

function StatusChip({ status, error }: { status: RowState["status"]; error?: string }) {
  if (status === "running") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border border-border bg-secondary px-2.5 py-0.5 text-xs font-medium text-secondary-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        查询中
      </span>
    );
  }
  if (status === "ok") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border border-jade/30 bg-jade/10 px-2.5 py-0.5 text-xs font-medium text-jade">
        <CheckCircle2 className="h-3 w-3" />
        成功
      </span>
    );
  }
  if (status === "failed") {
    return (
      <span
        className="inline-flex items-center gap-1 rounded-full border border-destructive/30 bg-destructive/10 px-2.5 py-0.5 text-xs font-medium text-destructive"
        title={error}
      >
        <XCircle className="h-3 w-3" />
        失败
      </span>
    );
  }
  return (
    <span className="inline-flex items-center rounded-full border border-border bg-surface-container px-2.5 py-0.5 text-xs font-medium text-muted">
      等待
    </span>
  );
}

export function InviteProfilePage() {
  const [tab, setTab] = useState<"show" | "verify">("show");
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [catalog, setCatalog] = useState<PtdSitePreset[]>([]);
  const [sitesError, setSitesError] = useState("");
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [filter, setFilter] = useState("");
  const [previewOpen, setPreviewOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [pasteText, setPasteText] = useState("");
  const [queued, setQueued] = useState<ParsedItem[]>([]);
  const [unmatched, setUnmatched] = useState<UnmatchedLink[]>([]);
  const [preregUsername, setPreregUsername] = useState<string | null>(null);
  const [preregEmail, setPreregEmail] = useState<string | null>(null);
  const [states, setStates] = useState<RowState[]>([]);
  const [running, setRunning] = useState(false);
  const [parseError, setParseError] = useState("");
  const abortRef = useRef(false);

  const eligibleSites = useMemo(
    () => sites.filter((site) => site.stats?.uid),
    [sites],
  );

  const visibleSites = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return eligibleSites;
    return eligibleSites.filter(
      (site) =>
        site.name.toLowerCase().includes(q) ||
        siteHost(site).toLowerCase().includes(q) ||
        (site.stats?.uid ?? "").includes(q) ||
        (site.stats?.username ?? "").toLowerCase().includes(q),
    );
  }, [eligibleSites, filter]);

  const loadSites = useCallback(async () => {
    setSitesError("");
    try {
      const [siteList, presets] = await Promise.all([
        api<SiteRecord[]>("/api/sites"),
        api<PtdSitePreset[]>("/api/sites/catalog").catch(() => [] as PtdSitePreset[]),
      ]);
      setSites(siteList);
      setCatalog(presets);
    } catch (error) {
      setSitesError((error as Error).message || "站点列表读取失败");
    }
  }, []);

  useEffect(() => {
    void loadSites();
  }, [loadSites]);

  useEffect(() => () => {
    abortRef.current = true;
  }, []);

  const selectedSites = useMemo(
    () => eligibleSites.filter((site) => selectedIds.includes(site.id) && site.stats?.uid),
    [eligibleSites, selectedIds],
  );

  const showTotals = useMemo(() => {
    let uploaded = 0;
    let downloaded = 0;
    let seeding = 0;
    let seedingSize = 0;
    for (const site of selectedSites) {
      uploaded += site.stats?.uploaded ?? 0;
      downloaded += site.stats?.downloaded ?? 0;
      seeding += site.stats?.seeding_count ?? 0;
      seedingSize += site.stats?.seeding_size ?? 0;
    }
    return { uploaded, downloaded, seeding, seedingSize, count: selectedSites.length };
  }, [selectedSites]);

  /** 从已选站点里取出现次数最多的用户名 / 邮箱作为预注册信息。 */
  const commonIdentity = useMemo(() => {
    const nameCount = new Map<string, number>();
    const emailCount = new Map<string, number>();
    for (const site of selectedSites) {
      const name = site.stats?.username?.trim();
      if (name) nameCount.set(name, (nameCount.get(name) ?? 0) + 1);
      const email = normalizeEmail(site.stats?.email ?? "");
      if (email && email !== "—") emailCount.set(email, (emailCount.get(email) ?? 0) + 1);
    }
    const pick = (map: Map<string, number>) => {
      let best: string | null = null;
      let bestN = 0;
      for (const [value, n] of map) {
        if (n > bestN || (n === bestN && best !== null && value < best)) {
          best = value;
          bestN = n;
        }
      }
      return best ? { value: best, count: bestN } : null;
    };
    return {
      username: pick(nameCount),
      email: pick(emailCount),
      nameTotal: selectedSites.length,
    };
  }, [selectedSites]);

  const copyPayload = useMemo(() => {
    const lines = [COPY_HEADER, LINE_COMMENT, ""];
    if (commonIdentity.username) {
      lines.push(`预注册ID：${commonIdentity.username.value}`);
    }
    if (commonIdentity.email) {
      lines.push(`邮箱：${commonIdentity.email.value}`);
    }
    if (commonIdentity.username || commonIdentity.email) {
      lines.push("");
    }
    for (const site of selectedSites) {
      const uid = site.stats?.uid ?? "";
      if (!uid) continue;
      lines.push(`${site.name}|${uid}`);
    }
    if (showTotals.count > 0) {
      lines.push(
        "",
        `合计：${showTotals.count} 站 · 上传 ${formatBytes(showTotals.uploaded)} · 下载 ${formatBytes(showTotals.downloaded)} · 做种 ${showTotals.seeding}（${formatBytes(showTotals.seedingSize)}）`,
      );
    }
    return lines.join("\n");
  }, [selectedSites, showTotals, commonIdentity]);

  const selectedCount = selectedIds.length;
  const doneCount = states.filter((s) => s.status === "ok" || s.status === "failed").length;
  const okCount = states.filter((s) => s.status === "ok").length;
  const failCount = states.filter((s) => s.status === "failed").length;

  function toggleSite(id: number) {
    setSelectedIds((current) =>
      current.includes(id) ? current.filter((item) => item !== id) : [...current, id],
    );
  }

  function toggleAllVisible() {
    const visibleIds = visibleSites.map((site) => site.id);
    const allSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedIds.includes(id));
    if (allSelected) {
      setSelectedIds((current) => current.filter((id) => !visibleIds.includes(id)));
    } else {
      setSelectedIds((current) => [...new Set([...current, ...visibleIds])]);
    }
  }

  async function runLookup(items: ParsedItem[]) {
    abortRef.current = false;
    setStates(items.map(() => ({ status: "pending" })));
    setRunning(true);
    for (let index = 0; index < items.length; index += 1) {
      if (abortRef.current) break;
      const item = items[index];
      setStates((current) =>
        current.map((state, i) => (i === index ? { ...state, status: "running" } : state)),
      );
      if (item.error) {
        setStates((current) =>
          current.map((state, i) =>
            i === index ? { status: "failed", error: item.error } : state,
          ),
        );
        continue;
      }
      try {
        const results = await api<LookupResult[]>("/api/invite-profile/lookup", {
          method: "POST",
          body: JSON.stringify({ items: [{ site_id: item.site_id, uid: item.uid }] }),
        });
        const result = results[0];
        if (!result) {
          setStates((current) =>
            current.map((state, i) =>
              i === index ? { status: "failed", error: "无返回结果" } : state,
            ),
          );
        } else if (result.failure) {
          setStates((current) =>
            current.map((state, i) =>
              i === index
                ? { status: "failed", result, error: result.message || result.failure }
                : state,
            ),
          );
        } else {
          setStates((current) =>
            current.map((state, i) => (i === index ? { status: "ok", result } : state)),
          );
        }
      } catch (error) {
        setStates((current) =>
          current.map((state, i) =>
            i === index
              ? { status: "failed", error: (error as Error).message || "查询失败" }
              : state,
          ),
        );
      }
    }
    setRunning(false);
  }

  /** 解析后立刻查询；输入框始终保留。 */
  async function handleParseAndLookup() {
    abortRef.current = true;
    const payload = parseInvitePayload(pasteText, sites, catalog);
    setPreregUsername(payload.preregister_username);
    setPreregEmail(payload.preregister_email);
    setUnmatched(payload.unmatched);
    if (!payload.items.length) {
      setParseError(
        payload.unmatched.length
          ? "只识别到链接/俗称，但本机都未配置对应站点。请先在「站点管理」添加站点。"
          : "未解析到有效条目。可粘贴云母管道格式、资料链接或「站名：UID」。",
      );
      setQueued([]);
      setStates([]);
      return;
    }
    setParseError("");
    setQueued(payload.items);
    await runLookup(payload.items);
  }

  function resetLookup() {
    abortRef.current = true;
    setQueued([]);
    setStates([]);
    setUnmatched([]);
    setPreregUsername(null);
    setPreregEmail(null);
    setParseError("");
    setRunning(false);
  }

  const tabs = [
    { key: "show", label: "出示求药信息" },
    { key: "verify", label: "核验对方资料" },
  ] as const;

  return (
    <div className="space-y-4">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
        <div className="min-w-0">
          <h1 className="text-xl font-semibold tracking-tight sm:text-2xl">求药 / 发药</h1>
          <p className="mt-1 text-sm leading-6 text-muted">
            出示站点 UID，或粘贴对方求药信息逐条核验。
          </p>
        </div>
        <div
          role="tablist"
          aria-label="求药发药"
          className="flex w-full gap-1 rounded-xl border border-border bg-card p-1 sm:w-auto"
        >
          {tabs.map((item) => (
            <button
              key={item.key}
              type="button"
              role="tab"
              id={`invite-tab-${item.key}`}
              aria-selected={tab === item.key}
              aria-controls="invite-tab-panel"
              className={cn(
                "min-h-11 flex-1 whitespace-nowrap rounded-lg px-3 text-sm font-semibold transition-colors sm:flex-none sm:px-5",
                tab === item.key
                  ? "bg-accent text-primary"
                  : "text-muted hover:bg-accent/60 hover:text-foreground",
              )}
              onClick={() => setTab(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </header>

      <div id="invite-tab-panel" role="tabpanel" aria-labelledby={`invite-tab-${tab}`}>
        {tab === "show" ? (
          <Card>
            <CardHeader className="pb-3">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0">
                  <CardTitle className="flex items-center gap-2 text-base">
                    <ListChecks className="h-4 w-4 text-primary" />
                    出示求药信息
                  </CardTitle>
                  <CardDescription className="mt-1">
                    勾选已有 UID 的站点。站名使用「站点管理」中的名称，不合适可先去改备注。
                  </CardDescription>
                </div>
                <Button
                  size="sm"
                  className="h-11 w-full sm:h-9 sm:w-auto"
                  disabled={!selectedCount}
                  onClick={() => setPreviewOpen(true)}
                >
                  <Eye className="mr-1.5 h-4 w-4" />
                  预览{selectedCount ? ` (${selectedCount})` : ""}
                </Button>
              </div>
            </CardHeader>
            <CardContent className="space-y-3">
              {sitesError ? (
                <p role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                  {sitesError}
                </p>
              ) : null}

              <div className="flex flex-wrap items-center gap-2">
                <div className="relative min-w-0 flex-1">
                  <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
                  <Input
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    placeholder="筛选站点 / UID / 用户名"
                    className="h-11 pl-9 sm:h-10"
                    aria-label="筛选站点"
                  />
                </div>
                <Button variant="outline" size="sm" className="h-11 flex-1 sm:h-10 sm:flex-none" onClick={toggleAllVisible} disabled={!visibleSites.length}>
                  {visibleSites.length > 0 && visibleSites.every((s) => selectedIds.includes(s.id))
                    ? "取消本页"
                    : "选中本页"}
                </Button>
                <Button variant="outline" size="sm" className="h-11 flex-1 sm:h-10 sm:flex-none" onClick={() => setSelectedIds([])} disabled={!selectedCount}>
                  清空
                </Button>
              </div>

              {eligibleSites.length === 0 ? (
                <div className="rounded-xl border border-border bg-surface-container px-4 py-8 text-center">
                  <Gift className="mx-auto h-8 w-8 text-muted" aria-hidden />
                  <p className="mt-3 text-sm font-medium">还没有可出示的站点</p>
                  <p className="mt-1 text-xs leading-5 text-muted">
                    先到「站点管理」刷新统计，获取 UID 后再回来。
                  </p>
                </div>
              ) : (
                <div className="overflow-x-auto rounded-xl border border-border">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead className="w-10">
                          <span className="sr-only">选择</span>
                        </TableHead>
                        <TableHead>站点</TableHead>
                        <TableHead className="w-16">ID</TableHead>
                        <TableHead className="w-28">UID</TableHead>
                        <TableHead>账户</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {visibleSites.map((site) => {
                        const checked = selectedIds.includes(site.id);
                        return (
                          <TableRow
                            key={site.id}
                            className={cn("cursor-pointer", checked && "bg-primary/5")}
                            onClick={() => toggleSite(site.id)}
                          >
                            <TableCell>
                              <input
                                type="checkbox"
                                checked={checked}
                                onChange={() => toggleSite(site.id)}
                                onClick={(event) => event.stopPropagation()}
                                className="h-4 w-4 accent-primary"
                                aria-label={`选择 ${site.name}`}
                              />
                            </TableCell>
                            <TableCell>
                              <div className="font-medium">{site.name}</div>
                              <div className="text-xs text-muted">{siteHost(site)}</div>
                            </TableCell>
                            <TableCell className="tabular-nums text-muted">{site.id}</TableCell>
                            <TableCell className="font-mono text-xs tabular-nums">{site.stats?.uid}</TableCell>
                            <TableCell>
                              <div className="truncate text-sm">{site.stats?.username || "—"}</div>
                              <div className="truncate text-xs text-muted">
                                {site.stats?.email || "邮箱未抓取"}
                              </div>
                            </TableCell>
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                </div>
              )}

              <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted">
                <span>
                  已选 <span className="font-semibold text-foreground tabular-nums">{selectedCount}</span>
                  {" / "}
                  <span className="tabular-nums">{eligibleSites.length}</span> 个可出示站点
                </span>
                {showTotals.count > 0 ? (
                  <span className="tabular-nums">
                    合计上传 {formatBytes(showTotals.uploaded)} · 下载 {formatBytes(showTotals.downloaded)} · 做种 {showTotals.seeding}（{formatBytes(showTotals.seedingSize)}）
                  </span>
                ) : null}
              </div>

              {selectedCount > 0 ? (
                <div className="rounded-xl border border-primary/25 bg-primary/5 px-3 py-2 text-xs leading-5">
                  <span className="font-medium text-muted">将写入的预注册信息（取所选站点中出现最多的）</span>
                  <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-sm">
                    <span>
                      <span className="text-xs text-muted">ID </span>
                      <span className="font-medium">
                        {commonIdentity.username?.value ?? "（无用户名统计）"}
                      </span>
                      {commonIdentity.username ? (
                        <span className="ml-1 text-xs text-muted">
                          {commonIdentity.username.count}/{selectedSites.length} 站
                        </span>
                      ) : null}
                    </span>
                    <span className="break-all">
                      <span className="text-xs text-muted">邮箱 </span>
                      <span className="font-medium">
                        {commonIdentity.email?.value ?? "（无邮箱统计）"}
                      </span>
                      {commonIdentity.email ? (
                        <span className="ml-1 text-xs text-muted">
                          {commonIdentity.email.count}/{selectedSites.length} 站
                        </span>
                      ) : null}
                    </span>
                  </div>
                </div>
              ) : null}

              <div className="rounded-xl border border-border bg-surface-container/80 px-3 py-2 text-xs leading-5 text-muted">
                站名取自「站点管理」中的站点名称；若不适合对外展示，可先到站点管理修改备注名。格式为
                <span className="font-mono text-foreground"> 站点名|UID</span>
                ，并附预注册 ID / 邮箱（所选站点中出现次数最多）及合计上传 / 下载 / 做种。
              </div>
            </CardContent>
          </Card>
        ) : (
          <Card>
            <CardHeader className="pb-3">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0">
                  <CardTitle className="flex items-center gap-2 text-base">
                    <ShieldCheck className="h-4 w-4 text-primary" />
                    核验对方资料
                  </CardTitle>
                  <CardDescription className="mt-1">
                    粘贴后自动解析并查询；输入框始终可编辑。
                  </CardDescription>
                </div>
                <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto">
                  {queued.length || unmatched.length ? (
                    <Button variant="outline" size="sm" className="h-11 flex-1 sm:h-9 sm:flex-none" disabled={running} onClick={resetLookup}>
                      清空结果
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    className="h-11 flex-1 sm:h-9 sm:flex-none"
                    disabled={running || !pasteText.trim()}
                    onClick={() => void handleParseAndLookup()}
                  >
                    {running ? (
                      <Loader2 className="mr-1.5 h-4 w-4 animate-spin" />
                    ) : (
                      <Search className="mr-1.5 h-4 w-4" />
                    )}
                    {running ? `查询 ${doneCount + (doneCount < queued.length ? 1 : 0)}/${queued.length || "…"}` : "解析并查询"}
                  </Button>
                </div>
              </div>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="space-y-1.5">
                <Label htmlFor="invite-paste">求药信息</Label>
                <textarea
                  id="invite-paste"
                  value={pasteText}
                  onChange={(event) => setPasteText(event.target.value)}
                  rows={10}
                  spellCheck={false}
                  className="w-full resize-y rounded-lg border border-border bg-card px-3 py-2 font-mono text-xs leading-5 text-foreground outline-none focus:border-primary focus:ring-2 focus:ring-primary/20"
                  placeholder={"预注册ID：example\n邮箱：user@example.com\nhttps://pt.example.com/userdetails.php?id=123\n猫：27701\n青蛙|5|708227"}
                />
                <p className="text-xs leading-5 text-muted">
                  支持管道格式、资料链接（userdetails / profile/detail）、圈内俗称（如「猫：27701」）。点「解析并查询」会立刻拉取。
                </p>
              </div>

              {parseError ? (
                <p role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                  {parseError}
                </p>
              ) : null}

              {(preregUsername || preregEmail) ? (
                <div className="rounded-xl border border-primary/25 bg-primary/5 px-3 py-2 text-sm">
                  <div className="text-xs font-medium text-muted">预注册信息</div>
                  <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
                    {preregUsername ? (
                      <span>
                        <span className="text-xs text-muted">ID </span>
                        <span className="font-medium">{preregUsername}</span>
                      </span>
                    ) : null}
                    {preregEmail ? (
                      <span className="break-all">
                        <span className="text-xs text-muted">邮箱 </span>
                        <span className="font-medium">{preregEmail}</span>
                      </span>
                    ) : null}
                  </div>
                </div>
              ) : null}

              {queued.length > 0 ? (
                <div className="rounded-xl border border-border bg-surface-container/70 px-3 py-2">
                  <div className="text-xs font-medium text-muted">已解析清单</div>
                  <pre className="mt-1 max-h-24 overflow-auto whitespace-pre-wrap font-mono text-[11px] leading-5 text-foreground">
                    {queued
                      .map((item) =>
                        item.error
                          ? `${item.raw}  ← ${item.error}`
                          : `${item.site_label || "站点" + item.site_id}|${item.site_id}|${item.uid}`,
                      )
                      .join("\n")}
                  </pre>
                </div>
              ) : null}

              {unmatched.length > 0 ? (
                <div className="rounded-xl border border-amber-500/40 bg-amber-500/10 px-3 py-2">
                  <div className="text-xs font-medium text-amber-800">
                    本机没有的站点（{unmatched.length}）
                  </div>
                  <ul className="mt-1 space-y-1 text-xs text-amber-900">
                    {unmatched.map((item) => (
                      <li key={`${item.host}-${item.uid}-${item.url}`} className="break-all font-mono">
                        {item.display_name || item.host}
                        {" · UID "}
                        {item.uid}
                        <span className="ml-1 font-sans opacity-80">— {item.reason}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {queued.length > 0 ? (
                <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
                  <span className="rounded-full border border-border bg-surface-container px-2.5 py-0.5 tabular-nums">
                    {doneCount}/{queued.length} 已完成
                  </span>
                  {okCount > 0 ? (
                    <span className="rounded-full border border-jade/30 bg-jade/10 px-2.5 py-0.5 font-medium text-jade tabular-nums">
                      成功 {okCount}
                    </span>
                  ) : null}
                  {failCount > 0 ? (
                    <span className="rounded-full border border-destructive/30 bg-destructive/10 px-2.5 py-0.5 font-medium text-destructive tabular-nums">
                      失败 {failCount}
                    </span>
                  ) : null}
                </div>
              ) : null}

              {queued.length > 0 ? (
                <ul className="divide-y divide-border overflow-hidden rounded-xl border border-border">
                  {queued.map((item, index) => {
                    const state = states[index] ?? { status: "pending" as const };
                    const result = state.result;
                    const showAs =
                      item.site_label || result?.site_name || `站点 ${item.site_id}`;
                    return (
                      <li key={`${item.raw}-${index}`} className="bg-card px-3 py-3">
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <div className="min-w-0">
                            <div className="text-sm font-medium">
                              {showAs}
                              <span className="ml-2 font-mono text-xs tabular-nums text-muted">
                                ID {item.site_id || "—"} · UID {item.uid || "—"}
                              </span>
                            </div>
                            <p className="mt-0.5 break-all font-mono text-[11px] text-muted">{item.raw}</p>
                          </div>
                          <StatusChip status={state.status} error={state.error} />
                        </div>
                        {item.error ? (
                          <p role="status" className="mt-2 text-sm text-destructive">
                            {item.error}
                          </p>
                        ) : null}
                        {state.error && state.status === "failed" ? (
                          <p role="status" className="mt-2 text-sm text-destructive">
                            {state.error}
                          </p>
                        ) : null}
                        {result && state.status === "ok" ? (
                          <div className="mt-2 grid gap-2 sm:grid-cols-2">
                            <div>
                              <div className="text-sm font-medium">
                                {result.site_name || showAs}
                                <span className="ml-1.5 text-xs font-normal text-muted">
                                  {result.site_type}
                                </span>
                              </div>
                              <div className="mt-0.5 font-mono text-xs tabular-nums text-muted">
                                UID {result.uid}
                                {result.username ? ` · ${result.username}` : ""}
                              </div>
                              {preregUsername || preregEmail ? (
                                <div className="mt-1.5 flex flex-wrap gap-1.5 text-xs">
                                  {preregUsername ? (
                                    <span
                                      className={cn(
                                        "rounded-full border px-2 py-0.5",
                                        result.username &&
                                          result.username.toLowerCase() === preregUsername.toLowerCase()
                                          ? "border-jade/30 bg-jade/10 text-jade"
                                          : "border-amber-500/40 bg-amber-500/10 text-amber-800",
                                      )}
                                    >
                                      用户名{" "}
                                      {result.username &&
                                      result.username.toLowerCase() === preregUsername.toLowerCase()
                                        ? "与预注册一致"
                                        : result.username
                                          ? "与预注册不同"
                                          : "未获取"}
                                    </span>
                                  ) : null}
                                  {preregEmail ? (
                                    <span
                                      className={cn(
                                        "rounded-full border px-2 py-0.5",
                                        result.email &&
                                          normalizeEmail(result.email) === preregEmail
                                          ? "border-jade/30 bg-jade/10 text-jade"
                                          : "border-amber-500/40 bg-amber-500/10 text-amber-800",
                                      )}
                                    >
                                      邮箱{" "}
                                      {result.email && normalizeEmail(result.email) === preregEmail
                                        ? "与预注册一致"
                                        : result.email
                                          ? "与预注册不同"
                                          : "未公开"}
                                    </span>
                                  ) : null}
                                </div>
                              ) : null}
                            </div>
                            <div className="text-sm sm:text-right">
                              <div className="break-all text-xs">{result.email || "邮箱未公开"}</div>
                              <div className="mt-0.5 text-xs tabular-nums text-muted">
                                {formatBytes(result.uploaded)} / {formatBytes(result.downloaded)}
                                {" · 分享率 "}
                                {result.ratio != null ? result.ratio.toFixed(3) : "—"}
                              </div>
                              <div className="mt-0.5 text-xs tabular-nums text-muted">
                                做种{" "}
                                {result.seeding_count != null ? result.seeding_count : "—"}
                                {"（"}
                                {formatBytes(result.seeding_size)}
                                {"） · 入站 "}
                                {formatJoinTime(result.join_time)}
                              </div>
                            </div>
                          </div>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              ) : null}
            </CardContent>
          </Card>
        )}
      </div>

      <Dialog
        open={previewOpen}
        onClose={() => setPreviewOpen(false)}
        title="求药信息预览"
        description="确认内容后再复制发给对方。站名来自站点管理。"
        footer={
          <div className="flex flex-col gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" className="h-11 sm:h-10" onClick={() => setPreviewOpen(false)}>
              关闭
            </Button>
            <Button
              className="h-11 sm:h-10"
              disabled={!copyPayload.trim() || copyPayload === `${COPY_HEADER}\n${LINE_COMMENT}\n`}
              onClick={async () => {
                await copyText(copyPayload);
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1600);
              }}
            >
              {copied ? <Check className="mr-1.5 h-4 w-4" /> : <Copy className="mr-1.5 h-4 w-4" />}
              {copied ? "已复制" : "复制"}
            </Button>
          </div>
        }
      >
        <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap rounded-xl border border-border bg-surface-container p-3 font-mono text-xs leading-5 text-foreground">
          {copyPayload}
        </pre>
      </Dialog>
    </div>
  );
}
