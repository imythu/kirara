# 自动追剧与资源搜索系统设计

## 1. 设计结论

本设计在现有 kirara 架构上新增独立的媒体自动化域，不替换现有刷流、站点统计、签到或下载器流程。

核心约束：

- 保留 `sites`、`auth_config`、`SiteAdapter` 和现有站点管理 API。
- 保留 `DownloaderClient`、`DownloaderClientPool` 和 qBittorrent 配置。
- 新增 `IndexerAdapter`，将搜索和种子获取与站点统计职责分离。
- 资源解析和决策是纯逻辑；网络、数据库和调度只负责编排。
- 错误媒体、季、集以及质量拒绝属于硬门槛，不能靠总分抵消。
- 下载采用“至少一次投递 + infohash 幂等 + 状态对账”，不伪装成跨 SQLite 与 qBittorrent 的 exactly-once 事务。
- 手动资源搜索、手动下载与自动追剧复用同一搜索、解析、决策和下载队列。

架构评审结论：修订后通过，可以进入实现阶段。

## 2. 研究依据

### 2.1 当前项目

- `src/site/mod.rs` 已提供稳定的 `SiteRecord`、`SiteAuth`、`SiteAdapter`。
- `src/site/nexusphp.rs` 已实现 Cookie 请求、过期识别和详情页解析。
- `src/site/mteam.rs` 已实现 `x-api-key`、API 错误处理与促销解析。
- `src/downloader/mod.rs` 已提供可扩展的下载器 trait 和配置变更自动失效的连接池。
- `src/db.rs` 使用 SQLite WAL、foreign keys、busy timeout 和启动时幂等迁移。
- `src/brush/scheduler.rs` 已提供可参考的后台调度、任务防重和下载器调用模式。

### 2.2 pt_mate

本地参考：`pt_mate` commit `d5c5904e`。

重点源码：

- `lib/services/api/site_adapter.dart`：统一站点能力接口与工厂。
- `lib/services/api/api_service.dart`：适配器生命周期和指定站点调用。
- `lib/services/aggregate_search_service.dart`：多站并发、单站错误隔离、进度与结果归一化。
- `lib/services/api/nexusphp_adapter.dart`：Bearer API、`/api/v1/torrents`、结果字段映射。
- `lib/services/api/nexusphp_web_adapter.dart`：Cookie 注入、登录重定向识别、配置化网页搜索与种子中转下载。
- `lib/services/api/mteam_adapter.dart`：`/api/torrent/search`、`/api/torrent/genDlToken` 和 M-Team 字段映射。

迁移原则：复用请求和归一化思想，不复制 Flutter 状态管理、WebView 登录和前端存储实现。kirara 继续使用已有 `sites.auth_config`，不引入第二套站点配置。

### 2.3 Sonarr 与 GuessIt

本地参考：Sonarr commit `9a94807a`，GuessIt commit `ee6c43b0`。

重点源码：

- Sonarr `Parser.cs`、`QualityParser.cs`、`ParsingService.cs`。
- Sonarr `DownloadDecisionMaker.cs`、`DecisionEngine/Specifications/*`。
- Sonarr `QualityProfile.cs`、`DownloadDecisionComparer.cs`。
- Sonarr `ReleaseSearchService.cs` 和 typed search criteria。
- GuessIt `episodes.py`、`title.py`、`type.py` 的中文标记和冲突消解。

迁移原则：

- 使用有序解析规则和显式中间模型，不直接复制依赖 .NET lookbehind、backreference 和重复 capture 的正则。
- 将 `Parse -> Identity Gate -> Decision -> Rank` 分层。
- 候选先收集稳定拒绝码，再对合格结果排序。
- 质量枚举与用户质量顺序分离。
- 电影使用独立解析路径；国产 `第N季/第N集/第N话` 和动画绝对集使用专门规则。

## 3. 系统架构图

```text
React 自动追剧工作台
  |  TMDB 搜索 / 订阅管理 / 资源搜索 / 质量配置 / 下载状态
  v
Axum /api/media/*
  |
  v
Media Application Service
  +--> TMDB Client --------------------------> TMDB API
  +--> Query Generator
  +--> Indexer Aggregator
  |      +--> NexusPHP API Adapter ----------> PT API
  |      +--> NexusPHP HTML Adapter ---------> PT Web + Cookie
  |      +--> M-Team Adapter ----------------> M-Team API
  +--> Release Parser
  +--> Identity Gate + Decision Specs
  +--> Stable Ranker
  +--> Subscription Repository
  +--> Download Outbox Repository
             |
             v
       Download Worker
         +--> IndexerAdapter.fetch_torrent
         +--> torrent infohash
         +--> DownloaderClientPool
                    |
                    v
               qBittorrent

Media Scheduler --> lease due subscriptions --> Media Application Service
Reconciler ------> lease uncertain downloads -> qBittorrent state / retry

SQLite
  sites/auth_config (existing)     downloaders (existing)
  media_settings                   quality_profiles
  subscriptions                    subscription_sites
  subscription_targets             media_downloads
```

## 4. 模块划分

### 4.1 `media::domain`

纯逻辑，无数据库和 HTTP：

- `release`：`ReleaseInfo`、有序解析规则、质量字段解析。
- `target`：电影、标准剧集、动画绝对集的 typed target。
- `query`：根据标题、别名、季、集生成分层搜索词。
- `decision`：硬拒绝、可解释评分、接受阈值。
- `quality`：质量配置、质量 rank 和拒绝规则。
- `rank`：确定性的 `SortKey`。

### 4.2 `indexer`

- `IndexerAdapter`：`search`、`fetch_torrent`、能力描述。
- `NexusPhpIndexer`：API Key 走 `/api/v1/torrents`；Cookie 走 NexusPHP HTML；登录页重定向或 HTML 登录特征统一报认证过期。
- `MTeamIndexer`：`x-api-key` 搜索、生成下载 token、下载种子。
- `IndexerPool`：按站点配置、代理和认证摘要缓存；按规范化 origin 共享访问 gate，配置变化重建 adapter 时不重置限速状态。搜索和取种操作共用单并发 gate，每次真实 HTTP 请求间隔至少 1 秒；收到 429 或明确的响应体频控错误后，按 `Retry-After` 或至少 60 秒冷却，冷却期间不访问远端。
- `IndexerAggregator`：不同站点 origin 按设置并发、同一 origin 的查询严格串行；单站错误隔离、跨查询去重，返回结果和逐站错误。

上层只接收统一模型：

```rust
struct SearchResult {
    site_id: i64,
    source_site: String,
    torrent_id: String,
    title: String,
    detail_url: Option<String>,
    download_locator: Option<String>,
    magnet: Option<String>,
    size: u64,
    seeders: u32,
    leechers: u32,
    publish_time: Option<DateTime<Utc>>,
}
```

`download_locator` 只能保存站内相对路径、种子 ID 或清洗后的公开定位符，不能保存 Cookie、API Key 或长期签名 URL。

### 4.3 `media::tmdb`

- 支持 v3 API Key 和 v4 Read Token。
- 搜索 `movie`、`tv` 和 `multi`。
- 获取 TV/电影详情、别名和季集信息。
- 返回本地稳定模型，不把 TMDB 原始 JSON 传到业务层。

### 4.4 `media::application`

- `MediaSearchService`：查询生成、聚合、解析、决策、排序。
- `SubscriptionService`：CRUD、目标生成、手动执行。
- `DownloadService`：幂等入队、取种子、infohash、提交、状态推进。
- `ReconciliationService`：处理外部调用结果未知和进程重启恢复。

### 4.5 `media::scheduler`

- 定期认领到期订阅，不包含匹配规则。
- 使用数据库 lease 和 version/CAS，避免手动执行与定时执行重复。
- 定期认领 outbox，并恢复过期的 `fetching/submitting` 项。

## 5. 数据模型

### 5.1 `media_settings`

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK, `id=1` | 单例 |
| `tmdb_token` | TEXT NULL | API Key 或 Read Token |
| `tmdb_language` | TEXT NOT NULL | 默认 `zh-CN` |
| `scan_interval_mins` | INTEGER NOT NULL | 默认 30 |
| `max_search_queries` | INTEGER NOT NULL | 默认 8 |
| `search_concurrency` | INTEGER NOT NULL | 默认 4；表示并发搜索的不同站点 origin 数，同 origin 请求始终串行限速 |
| `updated_at` | TEXT NOT NULL | 更新时间 |

### 5.2 `quality_profiles`

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK | 主键 |
| `name` | TEXT UNIQUE | 配置名 |
| `resolution_order` | TEXT JSON | 从高到低偏好 |
| `allowed_resolutions` | TEXT JSON | 允许集合 |
| `blocked_resolutions` | TEXT JSON | 明确拒绝 |
| `source_order` | TEXT JSON | WEB-DL、BluRay 等偏好 |
| `allowed_sources` | TEXT JSON | 允许来源 |
| `codec_order` | TEXT JSON | H265、H264、AV1 等偏好 |
| `blocked_codecs` | TEXT JSON | 明确拒绝 |
| `allow_unknown_quality` | INTEGER | 未识别质量是否允许 |
| `minimum_score` | INTEGER | 默认 80 |
| `min_seeders` | INTEGER | 最低做种数 |
| `created_at/updated_at` | TEXT | 审计字段 |

被活动订阅引用时禁止删除；可以先更新订阅或停用配置。

### 5.3 `subscriptions`

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK | 主键 |
| `tmdb_id` | INTEGER NOT NULL | TMDB ID |
| `media_type` | TEXT NOT NULL | `tv` / `movie` |
| `title` | TEXT NOT NULL | 主标题 |
| `original_title` | TEXT NULL | 原始标题 |
| `aliases_json` | TEXT NOT NULL | 解析和搜索别名 |
| `year` | INTEGER NULL | 首播/上映年份 |
| `poster_path` | TEXT NULL | TMDB 海报 |
| `season` | INTEGER NULL | 电影为空 |
| `next_episode` | INTEGER NULL | 电影为空 |
| `start_episode` | INTEGER NULL | 用户起始集 |
| `absolute_episode` | INTEGER NULL | 动画绝对集，可选 |
| `quality_profile_id` | INTEGER FK RESTRICT | 质量配置 |
| `downloader_id` | INTEGER FK RESTRICT | 下载器 |
| `save_path` | TEXT NULL | 保存路径 |
| `enabled` | INTEGER | 是否调度 |
| `next_run_at` | TEXT | 下次扫描 |
| `lease_owner/lease_until` | TEXT | 调度认领 |
| `version` | INTEGER | CAS 版本 |
| `last_status/last_error/last_run_at` | TEXT | 用户可见状态 |
| `created_at/updated_at` | TEXT | 审计字段 |

唯一索引使用 `media_type, tmdb_id, COALESCE(season, -1)`，避免同季重复订阅。

### 5.4 `subscription_sites`

| 字段 | 类型 | 说明 |
|---|---|---|
| `subscription_id` | INTEGER FK CASCADE | 订阅 |
| `site_id` | INTEGER FK RESTRICT | 现有站点 |
| `priority` | INTEGER | 站点排序 |

主键：`(subscription_id, site_id)`。不使用无法维护外键的 `site_ids` JSON。

### 5.5 `subscription_targets`

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK | 主键 |
| `subscription_id` | INTEGER FK CASCADE | 订阅 |
| `target_key` | TEXT NOT NULL | 规范化非空键 |
| `season/episode/absolute_episode` | INTEGER NULL | 目标编号 |
| `air_date` | TEXT NULL | TMDB 播出日期 |
| `status` | TEXT | `metadata_pending/pending/queued/submitted/skipped` |
| `created_at/updated_at` | TEXT | 审计字段 |

唯一键：`(subscription_id, target_key)`。

- 电影：`movie:{tmdb_id}`。
- 标准剧：`tv:{tmdb_id}:s02e03`。
- 动画绝对集：`tv:{tmdb_id}:abs0123`。

创建 TV 订阅时会在同一事务中物化 TMDB 已确认的季集与 `air_date`。只有
`pending` 目标可以进入 PT 搜索；`metadata_pending` 是尚未被 TMDB 确认的唯一前沿，
`skipped` 前沿记录 TMDB 已提供的季终证据。下载提交后从已物化目标中推进，未来集按
播出日期调度，季终时原子停用订阅，避免无限生成并搜索不存在的下一集。

### 5.6 `media_downloads`

持久化下载 outbox 和下载历史：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK | 主键 |
| `subscription_id` | INTEGER FK NULL | 手动下载可为空 |
| `target_key` | TEXT NOT NULL | 非空目标键 |
| `dedupe_key` | TEXT NOT NULL UNIQUE | 自动/手动统一幂等键 |
| `site_id/downloader_id` | INTEGER FK NULL | 删除后历史仍保留 |
| `source_site/downloader_name` | TEXT | 名称快照 |
| `torrent_id/title/size` | 基础字段 | 清洗后的发布信息 |
| `release_json` | TEXT | 不含凭据的归一化结果 |
| `decision_json` | TEXT | 解析、评分和拒绝明细 |
| `profile_snapshot_json` | TEXT | 当次质量规则快照 |
| `infohash` | TEXT UNIQUE NULL | qB 幂等与对账 |
| `status` | TEXT | 状态机 |
| `attempts` | INTEGER | 尝试次数 |
| `next_attempt_at` | TEXT NULL | 退避时间 |
| `lease_owner/lease_until` | TEXT NULL | worker 认领 |
| `version` | INTEGER | CAS 版本 |
| `last_error` | TEXT NULL | 最近错误 |
| `created_at/updated_at/submitted_at` | TEXT | 审计字段 |

状态机：

```text
queued -> fetching -> submitting -> submitted
   |          |            |
   +------> retry_wait <----+
                  |
                  +--> fetching

submitting -- 外部结果未知 --> reconciling --> submitted
                                      |       -> retry_wait
                                      +------ -> failed

queued/retry_wait -> cancelled
超过最大尝试次数 -> failed
```

所有状态跃迁使用 `WHERE id=? AND status=? AND version=?`。进程启动时，过期 lease 的非终态任务可恢复。

## 6. Release Parser 与 Decision Engine

### 6.1 `ReleaseInfo`

```rust
struct ReleaseInfo {
    raw_title: String,
    title: String,
    alternate_titles: Vec<String>,
    year: Option<u32>,
    season: Option<u32>,
    episodes: Vec<u32>,
    absolute_episodes: Vec<u32>,
    full_season: bool,
    resolution: Option<String>,
    codec: Option<String>,
    source: Option<String>,
    hdr_formats: Vec<String>,
    bit_depth: Option<u8>,
    revision: Option<String>,
    release_group: Option<String>,
    matched_rule: String,
}
```

有序规则优先级：

1. `S01E02`、`1x02`、多集与闭区间。
2. 中文 `第N季`、`第N集/话`。
3. 动画字幕组 + 绝对集、绝对集范围。
4. 季包/全集。
5. 电影标题 + 年份。
6. 独立质量、编码、来源、HDR / Dolby Vision、位深和 revision 解析。

范围展开必须限制最大长度，反向范围直接拒绝。数字标题如 `The 100`、`3x3 Eyes`、`Blade Runner 2049` 必须有专门回归测试。

### 6.2 Identity Gate

稳定拒绝码至少包含：

- `wrong_media`
- `wrong_title`
- `wrong_year`
- `wrong_season`
- `wrong_episode`
- `ambiguous_numbering`
- `season_pack_not_allowed`
- `quality_not_allowed`
- `unknown_quality`
- `minimum_seeders`

标题、季、集、媒体类型和显式质量禁用属于永久拒绝。临时做种数不足可记录为临时拒绝，下一轮允许重新检查。

### 6.3 可解释评分

只有通过 Identity Gate 才计算：

| 项目 | 满分 |
|---|---:|
| 标题匹配 | 40 |
| 年份匹配 | 10 |
| 季匹配 | 20 |
| 集匹配 | 20 |
| 质量偏好 | 10 |

`MatchDecision` 返回总分、各项得分、拒绝码和用户可读说明。默认自动接受要求 `score >= profile.minimum_score` 且没有永久拒绝。

### 6.4 稳定排序

```text
accepted DESC,
resolution_rank DESC,
source_rank DESC,
size_fitness DESC,
video_feature_rank DESC,
codec_rank DESC,
seeders DESC,
score DESC,
publish_time DESC,
site_priority ASC,
stable_release_key ASC
```

`size_fitness` 按媒体类型、分辨率、集数和编码效率计算 0..1000 的体积充足度，达到参考体积后封顶，
避免把文件大小当成无限增益。只有 `size_fitness = 1000` 时才启用 `video_feature_rank`，依次区分
Dolby Vision（带 HDR 回退优先）、HDR10+、HDR10、HLG / HDR 和 10bit。片长不可用时体积充足度只是软排序信号，
不作为拒绝条件。最后的稳定键保证不同网络返回顺序不会改变最佳资源。

## 7. 搜索查询生成

使用 typed criteria：

```rust
enum SearchCriteria {
    Movie { titles, year },
    Episode { titles, season, episode },
    Anime { titles, absolute_episode, season_episode },
    Season { titles, season },
}
```

标准剧集示例：

```text
百日成王 S01E03
百日成王 S1E3
百日成王 03
Original Title S01E03
```

电影示例：

```text
沙丘2 2024
Dune Part Two 2024
```

每个 query 有 tier 和来源标题；限制最大 query 数并去重。标准剧集和动画使用两阶段搜索，并在总查询预算内为回退预留名额：先搜索带季集或绝对集编号的关键词；只要第一阶段至少有一次成功请求、但没有候选通过完整匹配规则，才继续搜索不带集数的标题。纯标题结果仍须通过标题、季、集数和质量规则，网络或鉴权全部失败时也不会被误判为“无结果”而追加请求。电影和整季搜索保持单阶段。

## 8. API 设计

### 设置与 TMDB

- `GET /api/media/settings`
- `PUT /api/media/settings`
- `GET /api/media/tmdb/search?query=...&media_type=multi|tv|movie`
- `GET /api/media/tmdb/details?tmdb_id=...&media_type=tv|movie`
- `GET /api/media/tmdb/season?tmdb_id=...&season=...`

### 质量配置

- `GET /api/media/quality-profiles`
- `POST /api/media/quality-profiles`
- `PUT /api/media/quality-profiles/{id}`
- `DELETE /api/media/quality-profiles/{id}`

### 订阅

- `GET /api/media/subscriptions`
- `POST /api/media/subscriptions`
- `PUT /api/media/subscriptions/{id}`
- `DELETE /api/media/subscriptions/{id}`
- `POST /api/media/subscriptions/{id}/run`
- `POST /api/media/subscriptions/{id}/pause`
- `POST /api/media/subscriptions/{id}/resume`
- `GET /api/media/subscriptions/{id}/downloads`

创建订阅请求只传 TMDB ID、媒体类型和用户规则；后端重新读取 TMDB 详情并持久化标题/别名，避免信任前端拼装的 metadata。

### 资源搜索与下载

- `POST /api/media/resources/search`
- `POST /api/media/downloads`
- `GET /api/media/downloads/{id}`
- `DELETE /api/media/downloads/{id}?version=...`
- `GET /api/media/downloads?subscription_id=...&status=...`

搜索响应包含：归一化站点结果、`ReleaseInfo`、`MatchDecision`、稳定排序位置和逐站错误。任何响应都不得返回站点凭据。

手动下载也进入同一 outbox。用户覆盖硬拒绝时必须显式提交 `override_reason`，并写入审计信息。

删除下载记录只清理 kirara 本地的终态 outbox/自动复制历史，不调用 qB 或 OpenList。关联下载、订阅扫描或迁移仍在运行时必须拒绝删除；TV 订阅可在编辑时显式清理所选集及之后的终态历史，把对应 `submitted` target 恢复为可搜索状态。

## 9. 调用流程

### 9.1 创建订阅

```text
用户搜索 TMDB
 -> 选择影视
 -> 设置季/起始集/质量/站点/下载器
 -> 后端重新获取 TMDB 详情与别名
 -> 事务写 subscriptions + subscription_sites + first target
 -> 设置 next_run_at=now
```

### 9.2 自动扫描

```text
Scheduler 原子认领 due subscription
 -> 刷新/读取目标信息
 -> QueryGenerator
 -> IndexerAggregator 多站搜索
 -> 去重
 -> ReleaseParser
 -> IdentityGate
 -> DecisionSpecs + Score
 -> StableRank
 -> claimed IMMEDIATE 事务校验 owner/version/lease/current target/readiness
 -> 同事务幂等写入 media_downloads(queued) 并完成 target pending -> queued
 -> owner/version CAS 结束扫描并更新 subscription.next_run_at，但不推进 episode
```

### 9.3 下载与推进

```text
Worker CAS 认领 queued/retry_wait
 -> IndexerAdapter.fetch_torrent
 -> 校验 bencode 并计算 infohash
 -> qBittorrent add_torrent
 -> 成功或 duplicate
 -> SQLite 事务：download=submitted、target=submitted、subscription CAS 推进
```

若 qB 请求结果未知，进入 `reconciling`；根据 infohash 查询 qB 后再标成功或重试。电影成功后订阅进入 `completed`；剧集生成下一 target。

### 9.4 手动资源搜索

```text
用户输入关键词或选择订阅目标
 -> 同一 Search/Parse/Decision/Rank pipeline
 -> 服务端为候选签发短期 opaque candidate_id，并缓存权威 SearchResult/target/profile
 -> 展示接受与拒绝原因
 -> 用户仅提交 candidate_id 和下载配置
 -> 服务端恢复权威候选并重新执行 Parse/Decision
 -> 关联订阅时原子校验 version/current target/pending/air_date/lease
 -> 同一事务写入 outbox 并完成 target pending -> queued
```

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| PT 页面结构差异 | API 优先；HTML 使用 Nexus 默认解析和 fixture；单站失败隔离 |
| Cookie 过期 | 检测登录跳转/登录 HTML，返回稳定认证错误，不吞掉 |
| 追剧 PT 搜索/取种限流 | 按 origin 共享单并发 gate、最小请求间隔、429/响应体频控冷却和逐站错误隔离 |
| 查询爆炸 | query 上限、并发上限、结果去重和超时 |
| 错剧误下 | Identity Gate 硬拒绝，评分不能覆盖 |
| 动画/数字标题歧义 | 有序规则、置信来源、反例 fixture |
| qB 提交崩溃窗口 | infohash 幂等、reconciling、at-least-once |
| 双 Scheduler/手动并发 | subscription lease、outbox lease、CAS、唯一幂等键 |
| 客户端伪造标题与种子 ID 绑定 | 短期随机 candidate_id、有界服务端缓存、入队时按权威结果重算 |
| 未播出/元数据待定目标被手动绕过 | 物化 target readiness；入队事务校验 pending、air_date 与终季证据 |
| 下载最终失败导致目标卡死 | failed 事务恢复 queued -> pending；未知外部提交保持 reconciling |
| 删除站点/下载器/质量配置 | 活动引用 RESTRICT；历史使用 nullable FK + 名称快照 |
| 凭据泄露 | outbox 与 API 只存 locator；序列化扫描测试 |
| SQLite 写竞争 | 保留 WAL/busy timeout/每连接 foreign_keys；短事务 |
| 破坏现有功能 | 新路由命名空间、新表、新 scheduler；现有 trait 不改签名 |

## 11. 修改文件列表

### 后端新增

- `src/indexer/mod.rs`
- `src/indexer/nexusphp.rs`
- `src/indexer/mteam.rs`
- `src/indexer/pool.rs`
- `src/media/mod.rs`
- `src/media/tmdb.rs`
- `src/media/domain/release.rs`
- `src/media/domain/query.rs`
- `src/media/domain/quality.rs`
- `src/media/domain/decision.rs`
- `src/media/domain/target.rs`
- `src/media/application/search.rs`
- `src/media/application/subscription.rs`
- `src/media/application/download.rs`
- `src/media/scheduler.rs`
- `src/db/media.rs`

### 后端修改

- `src/main.rs`：注册模块并启动/停止 media scheduler。
- `src/db.rs`：新增 schema、索引和 `db::media` 子模块。
- `src/web.rs`：注入 MediaService 并注册 `/api/media/*`。
- `src/downloader/mod.rs`：补充按 infohash 查询/重复成功语义所需的最小接口。
- `Cargo.toml` / `Cargo.lock`：仅在确有需要时增加 bencode/infohash 依赖。

### 前端新增

- `frontend/src/pages/media-page.tsx`
- 必要的 shadcn 源码组件（通过项目包管理器和 shadcn CLI 添加）。

### 前端修改

- `frontend/src/App.tsx`：新增导航和 lazy page。
- `frontend/src/types.ts`：媒体 API 类型。
- `frontend/src/lib/api.ts`：仅补充必要默认值或 helper。
- `frontend/src/index.css` / `tailwind.config.ts`：仅添加语义 token，不重做现有主题。

### 文档

- `doc/openapi.yaml`：补全新 API。
- `README.md`：配置和使用说明。

## 12. 实现与验收门槛

必须先完成：

1. 纯 parser/query/decision/quality 测试。
2. Nexus API/HTML 和 M-Team fixture 解析测试。
3. Schema、FK、唯一键、lease/CAS 和状态迁移测试。
4. 自动订阅到 qB 提交的完整 service 流程。
5. 手动搜索与下载复用同一 pipeline。
6. 前端创建订阅、查看匹配解释、手动下载和查看状态。

必须覆盖的反例：

- 双 scheduler 与手动/定时同时触发。
- lease 过期接管和每个 outbox 崩溃窗口。
- qB duplicate 和请求结果未知的对账。
- 电影 NULL 季集的幂等。
- 错季错集即使高分仍拒绝。
- 删除被引用的站点、下载器和质量配置。
- outbox/API 序列化内容不含 Cookie、API Key、密码。
- 稳定排序不受站点返回顺序影响。
- 现有 48 个 Rust 测试、前端生产构建和旧 API 回归继续通过。

可以后续扩展但不能污染本次边界：scene/XEM 映射、每日剧日期模式、整季升级、更多下载器、Gazelle/Unit3D、Cookie Cloud 和通知渠道。
