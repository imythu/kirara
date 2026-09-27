# 站点适配与 PTD 规则同步

本文说明 kirara 如何对齐 [PT-depiler](https://github.com/pt-plugins/PT-depiler) 的站点适配知识，以及站点改版后如何用数据规则跟进，而不是每个站从零手写适配器。

相关 Cookie 导入与 WebDAV 同步见 [WebDAV Cookie 自动同步](webdav-cookie-sync.md)。**Cookie 同步只更新登录凭据，不会更新解析规则。**

---

## 背景：为什么会有「拿不到数据」

| | PT-depiler | kirara |
| --- | --- | --- |
| 形态 | 浏览器扩展 | Rust 服务端（桌面 / Docker） |
| 站点模型 | Schema + 340 个 `definitions/*.ts` | 通用引擎适配器 + 全站规则快照 |
| 更新方式 | 随扩展 Release 发版；用户可 `merge` 覆盖 | 改代码 / 规则后编译发版 |
| 请求上下文 | 真实浏览器 Cookie、CF clearance | Cookie / API Key + reqwest |

PT-depiler 能修站，是因为**每个站点有独立 definition**（选择器、请求路径、自定义 class）。kirara 若只用通用 NexusPHP 标签解析，站点改版（字段改名、主题 class、魔力页路径变化）就会失败。

**Cookie 永远最新 ≠ 解析规则正确。**

---

## 架构总览

```text
PT-depiler definitions/*.ts (pinned commit)
        │
        │  tools/gen_ptd_site_rules.py
        ▼
tools/ptd_site_rules.json / .rs   （340 个 definition 的可审查快照）
        │
        │  生成运行时声明
        ▼
src/site/ptd_rules_generated.rs
        │
        ├─► NexusPHP / Unit3D 适配器（选择器 + process 请求）
        └─► rules.rs::SITE_RULES 手工覆盖；M-Team / Gazelle 使用专用适配器
```

### 站点类型（`src/site/mod.rs`）

| `site_type` | 用户统计 | 种子搜索 | 认证 |
| --- | --- | --- | --- |
| `nexusphp` | 有（规则可覆盖） | 有（`indexer/nexusphp.rs`） | Cookie / API Key / Passkey |
| `mteam` | 有 | 有 | **必须 API Key** |
| `gazelle` | 有 | **尚未支持** | Cookie |
| `unit3d` | 有 | 有（`indexer/unit3d.rs`） | **必须 Cookie** |

### 站点目录

- `src/ptd_site_catalog.rs`：可添加的站点预设（名称、类型、base_url、别名）
- `src/ptd_sites.rs`：host → PTD site id 映射（规则与备份识别用）

Unit3D 示例预设：`blutopia` / `aither` / `huno` / `fearnopeer` / `shareisland`。

---

## 站点规则 `SITE_RULES`

文件：`src/site/rules.rs`

`SITE_RULES` 保存手工覆盖；PTD 快照保存站点声明。运行时按字段优先读取手工规则，缺失字段再回退到生成规则。NexusPHP 与 Unit3D 适配器会按定义中的 `userInfo.process` 顺序请求页面，并使用每一步的响应类型和字段选择器：

1. 用 `base_url` 的 host 查 `ptd_sites::site_id_for_host`
2. 用 site id 查 `rules::rule_for_site`
3. 先使用站点字段选择器，缺失时回退通用标签解析
4. 依定义请求 profile、JSON、魔力页等流程；做种与发布数缺失时按 PTD 规则请求 AJAX

### 规则字段

| 字段 | 含义 |
| --- | --- |
| `ptd_id` | 与 PTD definition 的 `id` 一致 |
| `bonus_labels` 等 | 额外字段标签（优先于默认列表） |
| `*_selectors` | 标签失败后的 CSS 回退 |
| `bonus_page` | `Default` 或 `Path { path, query }`（支持 `{uid}`） |
| `user_torrent_ajax` | `disabled` + 额外请求头（如 Referer） |
| `json_user_stats` | JSON 用户信息接口（支持 `ptd` 与 `keepfrds` dialect） |
| `bonus_per_hour_selectors` | 资料页上直接给出的时魔选择器 |
| `user_info_processes` | 请求路径、查询参数、方法、响应类型及该步字段选择器 |

当前快照覆盖全部 340 个 definition；运行时生成 68 条含自定义数据的 NexusPHP / Unit3D 规则，其余这两类站点使用 PTD 通用 schema 流程。Gazelle 与 M-Team 使用独立适配器。其他 PTD schema 的 definition 保存在 JSON 快照中，但 kirara 尚未实现对应引擎。

### 已内置的高价值规则（示意）

| ptd_id | 要点 |
| --- | --- |
| `audiences` | 「爆米花」；顶栏 CSS 指标；AJAX 需 Referer |
| `byrbt` | 魔力页 `/mybonus.php?show=seed` |
| `u2` | UCoin；`/mprecent.php?user={uid}` |
| `keepfrds` | JSON `/api/userdetails.php`；`#perBonus`；禁用旧 AJAX |
| `ourbits` / `hdchina` / `hdsky` / `chdbits` | 标签或选择器补充 |

### 环境中的特例（可能不在规则表）

部分逻辑仍写在适配器里，例如 U2 的 UCoin `span[title]`、hhanclub 保种结算页。改这些站时要同时看 `src/site/nexusphp.rs` 与 `rules.rs`。

---

## Unit3D 支持

### 用户统计 `src/site/unit3d.rs`

- 首页提取用户名（`a[href*='/users/'][href*='settings']`）
- 详情页 `/users/{name}`：ratio-bar 与标签文本回退
- 收益页 `/users/{name}/earnings`：时魔
- 使用定义覆盖字段选择器和分步请求；HUNO、NordicBytes 等会按自己的 API / profile 路径读取
- 识别 Cloudflare / 登录页

### 种子搜索 `src/indexer/unit3d.rs`

- 请求：`GET /torrents/?name=…&perPage=…&page=…`
- 解析：新版 `torrent-search--list` 与旧版 `table-responsive` 列表
- 下载：`GET /torrents/download/{id}`（同源校验）
- `download_locator` 只存 torrent id，**不把签名 URL 写入搜索结果序列化**

### 能力边界

| 项目 | 状态 |
| --- | --- |
| Unit3D 用户统计 / 搜索 / 下载 | 已支持 |
| Unit3D 与各站主题差异 | 选择器覆盖常见布局；个别站仍可能需扩选择器 |
| Gazelle 搜索 | **尚未支持** |
| 规则远程热更新 | **尚未支持**（规则编译进二进制） |

---

## KeepFRDS JSON API

站点：`pt.keepfrds.com`（PTD id `keepfrds`）

- 主数据：`GET /api/userdetails.php`（JSON）
- 字段示意：`user.{id,username,class,joinedAt,uploadedBytes,downloadedBytes,bonus}`、`torrentStats.{seeding,seedingBytes,leeching,uploaded}`
- `user.class` 映射到等级 id / 名称（Peasant … Nexus Master / 贵宾 …）
- 时魔仍在 profile：`#info_block #perBonus` / `#perBonus`
- 已移除 `getusertorrentlistajax.php`，规则中 `ajax.disabled = true`

解析实现：`src/site/nexusphp.rs` 中 `parse_keepfrds_user_details_json`。

PTD 标准 JSON 字段路径可使用 `dialect: "ptd"`；KeepFRDS 使用专用解析。响应结构不同的 JSON 分站仍需扩展 dialect 解析，仅改路径不够。

---

## PTD definition → SITE_RULES 生成器

脚本：`tools/gen_ptd_site_rules.py`

### 职责

从 PT-depiler 的 TypeScript definition **抽取声明式数据**，生成：

| 产物 | 用途 |
| --- | --- |
| `tools/ptd_site_rules.json` | 全量结构化结果，便于 diff / review |
| `tools/ptd_site_rules.rs` | 全量 Rust 规则预览 |
| `src/site/ptd_rules_generated.rs` | NexusPHP / Unit3D 运行时规则，不手工编辑 |
| `tools/ptd_unit3d_presets.rs` | 可选的 Unit3D catalog 草稿 |

更新生成物后 review JSON 和 Rust diff；运行时规则由 `rules.rs` 自动查找，不需要粘贴进手工覆盖表。

### 可抽取的内容

- `schema`、`urls` / base_url
- 额外魔力标签（如「爆米花」）
- `userInfo.selectors` 中的 CSS 选择器
- `userInfo.process` 的请求路径、查询参数、方法、响应类型和字段选择器
- `/api/userdetails.php`
- AJAX `Referer` / `rot13` URL

### 抽不到的内容（必须手写）

- definition 里 **class 方法**（自定义解析、动态请求）
- 复杂 `elementProcess` / filter 函数
- 新站点引擎（Unit3D / Luminance / Avistaz 等）的完整适配
- Cloudflare、浏览器会话

### 用法

```powershell
# 从固定 PTD commit 生成全量快照和运行时规则
& $env:MIMO_PYTHON tools/gen_ptd_site_rules.py `
  --all `
  --commit 569a2a68cabbeeb1329d0e9ef52f6232e3723f8d `
  --json-out tools/ptd_site_rules.json `
  --rust-out tools/ptd_site_rules.rs `
  --runtime-out src/site/ptd_rules_generated.rs

# 指定站点
& $env:MIMO_PYTHON tools/gen_ptd_site_rules.py --site audiences --site byrbt

# 使用本地 PT-depiler 源码（改 definition 后立刻验证）
& $env:MIMO_PYTHON tools/gen_ptd_site_rules.py `
  --local <PT-depiler>/src/packages/site/definitions `
  --site keepfrds `
  --json-out tools/ptd_site_rules.json `
  --rust-out tools/ptd_site_rules.rs

# 固定 commit
& $env:MIMO_PYTHON tools/gen_ptd_site_rules.py --commit <sha> ...
```

---

## 标准工作流

```text
站点统计失败（Cookie 正常）
    → 查看 last_error，判断是标签 miss / 结构变化 / CF / 过期
    → 对照 PT-depiler definitions/<id>.ts（或最新 master / 本地修改）
    → 跑生成器
    → review tools/ptd_site_rules.json、.rs 和 runtime diff
    → 保留规则生成结果；只有运行时例外才添加 `SITE_RULES` 覆盖
    → cargo check --lib
    → 界面中「刷新站点统计」验证
```

### Review 检查清单

1. **ptd_id** 与 host 映射一致（`ptd_sites.rs`）
2. **选择器**字段未串台、未截断（`a[href*=` 这类无效）
3. **与默认标签重复**的可省略，避免干扰
4. **bonus_page / ajax / json** 与 PTD process 一致
5. **Referer** 域名与站点 base_url 一致
6. **Unit3D 站**不要塞进 NexusPHP 规则字段（走 Unit3D 适配器）
7. **已手调过的规则**（audiences / keepfrds 等）保留在 `SITE_RULES`，不要编辑生成文件
8. **空规则**（全 `&[]`）通常不必写入

### 验证

```powershell
cargo test -p kirara site::rules --lib
cargo test -p kirara site:: --lib
cargo test -p kirara indexer:: --lib
cargo test -p kirara --lib
```

可为新选择器补最小 HTML fixture（参考 `site::rules` / `indexer::unit3d` 测试）。

---

## 日常排错

| 现象 | 可能原因 | 处理 |
| --- | --- | --- |
| 没有找到上传量/下载量 | 标签改名或仅 CSS class | 规则加 selectors 或补标签 |
| 魔力/时魔为 0 | 魔力页路径或字段名变化 | 改 `bonus_page` / `bonus_per_hour_selectors` |
| 做种数一直空 | AJAX 端点删除或需要 Referer | `ajax.disabled` 或 `headers` |
| Unit3D 搜不到 | 站点类型未设为 `unit3d`，或 Cookie 无效 | 目录类型 + Cookie；看搜索错误 |
| FRDS 核心数据空、时魔空 | JSON 失败或 profile 选择器失效 | 查 `/api/userdetails.php` 与 `#perBonus` |
| Cloudflare 拦截 | 无头 HTTP 无浏览器会话 | 代理 / Cookie 更新；规则层解决不了 |
| Cookie 同步后仍失败 | 同步只改认证 | 仍需更新规则或适配器 |

---

## 与 PT-depiler 的关系（许可与边界）

- PT-depiler 为 **MIT** 许可；kirara 从其 definitions **提炼规则数据**，不是运行时加载其扩展代码。
- kirara **不复刻**浏览器扩展架构（content script、chrome.cookies、CF 过盾）。
- 适配目标是：**同一套适配知识 + Rust/规则引擎**，在无头请求前提下尽量提高采集成功率。
- 目录快照注释中的 commit 仅表示生成预设时的参考版本；规则应以当前 PT-depiler definition 为准。

---

## 维护约定

1. **能数据化就进 `SITE_RULES`**，优先于改通用解析。
2. **需要请求协议/JSON 方言/新引擎** 时，写适配器分支，并在规则中声明入口字段。
3. **生成器输出进仓库前必须 review**；可将 `tools/ptd_site_rules.json` 作为 PR 附件说明来源。
4. **不把手调规则无脑覆盖** 为生成片段。
5. **搜索与统计分离**：某站统计可用不代表搜索可用（如 Gazelle 搜索仍未支持）。
6. 规则目前**编译进二进制**；若将来做远程规则包，应先固定本文件中的 JSON schema 与 review 流程。

---

## 相关代码索引

| 路径 | 说明 |
| --- | --- |
| `src/site/rules.rs` | `SiteRule`、`SITE_RULES`、选择器提取 |
| `src/site/nexusphp.rs` | NexusPHP 统计 + 规则应用 + KeepFRDS JSON |
| `src/site/unit3d.rs` | Unit3D 用户统计 |
| `src/site/factory.rs` | site_type → 适配器 |
| `src/indexer/unit3d.rs` | Unit3D 搜索 / 下载 |
| `src/indexer/mod.rs` | `create_indexer` |
| `src/ptd_site_catalog.rs` | 站点预设 |
| `src/ptd_sites.rs` | host → ptd_id |
| `tools/gen_ptd_site_rules.py` | PTD definition → 规则草稿 |
| `doc/webdav-cookie-sync.md` | Cookie 同步（非解析规则） |

---

## 验证命令

```powershell
cargo test -p kirara --lib
# 可选：生成器
& $env:MIMO_PYTHON tools/gen_ptd_site_rules.py --site byrbt --json-out tools/ptd_site_rules.json
```
