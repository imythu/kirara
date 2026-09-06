# 云母

[![GitHub Release](https://img.shields.io/github/v/release/imythu/kirara?style=flat-square)](https://github.com/imythu/kirara/releases)
[![Docker Image](https://img.shields.io/github/v/release/imythu/kirara?style=flat-square&label=ghcr.io)](https://github.com/imythu/kirara/pkgs/container/kirara)

云母是一套面向 PT 用户的管理工具，提供自动追剧、电影订阅、多站资源搜索、qBittorrent 下载、刷流任务和站点数据总览。Windows 和 macOS 提供桌面应用，Linux 和 Docker 提供 Web 服务。

云母的英文项目名为 **Kirara**，仓库、二进制、Docker 镜像和新建数据库统一使用 `kirara`。项目已从 [imythu/rflush](https://github.com/imythu/rflush) 迁移至本仓库，后续更新、问题反馈和发布均在这里进行。

旧版本用户请先阅读[迁移指南](doc/migration-from-rflush.md)，保留原有数据目录即可继续使用。

## 主要功能

- 从 TMDB 搜索电影、电视剧和动漫并创建订阅
- 自动识别季、集、动画绝对集和已播出目标
- 并发搜索多个 NexusPHP、M-Team 站点
- 解析分辨率、片源和视频编码，按质量规则筛选排序
- 提供电视剧、电影、动漫的内置质量方案
- 手动搜索资源，查看匹配结果、拒绝原因和质量信息
- 自动或手动提交资源到 qBittorrent
- 使用持久化下载队列处理重试、去重和状态对账
- 管理 PT 刷流任务
- 使用公共 Lightpanda 或 Browserless 配置执行自动签到
- 查看 PT 站点上传量、下载量、分享率等账号数据
- 导出站点账号总览图片
- React Web 界面，适配桌面端和移动端

配置、订阅、下载记录和运行状态保存在本地 SQLite 数据库中，路径见[数据目录](#数据目录)。

## 快速开始

### Windows / macOS 桌面应用

从 [GitHub Releases](https://github.com/imythu/kirara/releases) 下载对应系统的安装包，并核对同一版本的 `SHA256SUMS.txt`：

| 系统 | 安装包 | 使用方式 |
| --- | --- | --- |
| Windows x64 | `kirara-2.3.3-x86_64-pc-windows-msvc-setup.exe` | 运行安装程序，完成后从开始菜单打开 Kirara |
| macOS Apple Silicon | `kirara-2.3.3-aarch64-apple-darwin.dmg` | 打开映像，将 Kirara 拖入“应用程序”，然后打开 |

桌面应用启动后直接显示管理界面，无需打开终端或浏览器。关闭窗口后，应用会隐藏到 Windows 系统托盘或 macOS 菜单栏，自动扫描、下载队列和定时任务继续在后台运行。

从托盘或菜单栏图标选择“打开主窗口”可恢复界面；再次启动 Kirara 也会打开原有窗口。需要停止服务时，请在图标菜单中选择“退出”（macOS 也可使用 `⌘Q`）。

当前 Windows 安装包未配置开发者签名，macOS 使用临时签名且未经过 Apple 公证，首次打开可能出现系统的发布者验证提示。macOS 可按 [Apple 的说明](https://support.apple.com/zh-cn/102445)在“隐私与安全性”中允许打开已确认来源的应用。

从 1.x 命令行版本升级时，请先按[桌面版数据迁移](#桌面版数据迁移)保留原有配置。

### Linux 直接运行

从 [GitHub Releases](https://github.com/imythu/kirara/releases) 下载对应架构的 `.tar.gz` 压缩包，解压后运行：

```bash
./kirara
```

默认访问地址：

```text
http://127.0.0.1:3000
```

常用启动参数：

```text
-H, --host <HOST>       监听地址，默认 127.0.0.1
-p, --port <PORT>       监听端口，默认 3000
-d, --data-dir <DIR>    数据库和运行数据目录
```

示例：

```bash
./kirara -H 127.0.0.1 -p 8080 -d ./runtime-data
```

对应环境变量为 `KIRARA_HOST`、`KIRARA_PORT` 和 `KIRARA_DATA_DIR`。

### Docker

正式版镜像支持 Linux `amd64` 和 `arm64`：

```bash
docker run --name kirara \
  -p 127.0.0.1:3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/imythu/kirara:2.3.3
```

自动构建提供 `latest-beta`（Linux amd64）；正式版提供 `latest`（Linux amd64 / arm64）。

指定版本：

```bash
docker run --name kirara \
  -p 127.0.0.1:3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/imythu/kirara:<version>
```

容器默认使用：

```text
KIRARA_HOST=0.0.0.0
KIRARA_PORT=3000
KIRARA_DATA_DIR=/data
```

## 首次配置

PTD 用户可点击站点管理中的「导入 PTD 配置」，按页面说明导出并选择 PTD 备份，手动导入站点 Cookie。也可在「站点管理 → 备份与同步 → Cookie 自动同步」启用内置 WebDAV 接收服务，让 PTD 定时推送后自动更新站点 Cookie。与现有 Web 服务共用端口，接收路径为 `/dav/ptd/`，首次保存生成独立连接密码；Docker 无需额外映射端口。配置方法、同步规则和部署说明见 [WebDAV Cookie 自动同步](doc/webdav-cookie-sync.md)。

建议按以下顺序完成配置：

1. 在“站点管理”中添加 PT 站点并测试连接。
2. 在“下载器”中添加 qBittorrent 并测试连接。
3. 在“自动追剧 -> 质量与设置”中填写 TMDB API Key 或 Read Access Token。
4. 检查自动扫描间隔、搜索并发和质量配置。
5. 从 TMDB 添加订阅，或进入“资源搜索”手动查找资源。

自动追剧和资源搜索复用同一套站点、下载器和质量配置，不需要重复维护账号。

## 自动追剧

### 创建订阅

1. 打开“自动追剧”。
2. 进入“TMDB 添加”。
3. 搜索影视名称并选择电影、电视剧或动漫。
4. 电视剧选择季和起始集；动漫可使用季集编号或绝对集编号。
5. 选择质量配置、搜索站点、下载器和保存路径。
6. 创建订阅。

创建后，系统会根据 TMDB 元数据确定当前应搜索的电影或剧集。尚未播出的集数不会提前下载；成功提交当前集后，订阅会推进到下一集。

订阅支持：

- 手动立即扫描
- 暂停和恢复
- 修改季、起始集、绝对集、站点、质量配置和下载器
- 查看最近一次扫描使用的搜索词、候选、站点错误和拒绝原因
- 查看关联下载任务及其状态

### 自动匹配流程

每次扫描会依次执行：

```text
生成搜索词
  -> 多站并发搜索
  -> 解析发布名称
  -> 校验影视标题、年份、季和集
  -> 应用质量规则
  -> 按匹配分、质量和做种数排序
  -> 将最佳资源加入下载队列
```

错剧、错季、错集、被禁止的质量和做种数不足属于明确拒绝条件，不会因为其它项目得分较高而被自动下载。

## 质量配置

新建质量配置时，可以直接选择内置方案，小白用户不需要填写专业参数：

| 类型 | 方案 | 主要偏好 |
| --- | --- | --- |
| 电视剧 | 日常 | 1080p WEB-DL 优先，兼顾更新速度和体积 |
| 电视剧 | 4K | 2160p WEB-DL 优先，1080p 作为备选 |
| 电影 | 收藏 | 2160p、REMUX、BluRay 优先 |
| 电影 | 均衡 | 1080p BluRay、WEB-DL 优先 |
| 动漫 | 日常 | 2160p 优先，兼容常见字幕组命名 |
| 动漫 | 省空间 | 1080p H.265、AV1 优先，拒绝 4K |

高级设置可以调整：

- 分辨率优先级、允许值和拒绝值
- 片源优先级和允许值，例如 `REMUX`、`BluRay`、`WEB-DL`、`WEBRip`
- 视频编码优先级和拒绝值，例如 `H265`、`H264`、`AV1`
- 最低匹配分
- 最低做种数
- 是否接受质量信息不完整的资源

资源标题中的 DIY、HDR、Dolby Vision、Dolby Atmos、10bit 等信息会保留供人工判断。当前自动质量筛选主要依据分辨率、片源和视频编码。

“恢复默认”会删除现有质量配置、重建六套内置方案，并把全部订阅切换到“电视剧 · 日常”。界面会进行两次后果确认，该操作不可撤销。

## 资源搜索与下载

“资源搜索”支持两种用法。

### 按关键词搜索

适合临时查找资源：

1. 输入影视名称、季集或其它关键词。
2. 选择一个或多个 PT 站点。
3. 可选质量配置，用于筛选和排序。
4. 点击搜索。
5. 查看候选后选择下载器并加入下载队列。

### 按订阅目标搜索

适合为当前追剧目标手动选种：

1. 在资源搜索中选择已有订阅。
2. 系统自动带入当前电影、季集或动画绝对集目标。
3. 搜索结果会经过与自动扫描相同的身份和质量校验。
4. 选择候选并下载。

这种方式比纯关键词搜索更严格，可以识别错剧、错季和错集。

### 理解搜索结果

每个候选会展示：

- 站点、标题、大小、做种数和发布时间
- 解析出的分辨率、片源、编码、季和集
- 是否通过自动匹配
- 匹配分和拒绝原因
- 下载状态

部分站点搜索失败时，其它站点的结果仍会返回。认证过期、限流和页面解析失败会分别展示，不会把“某个站点失败”误报为“没有资源”。

如果资源未通过规则，仍可手动覆盖，但必须填写覆盖原因。覆盖只影响本次下载，不会修改质量配置。

### 下载队列

自动追剧和手动搜索共用同一下载队列。任务会经历取种、校验、提交、对账、完成或失败等状态。

系统会：

- 解析 `.torrent` 并计算真实 infohash
- 按下载器和 infohash 防止重复提交
- 在网络失败或临时错误后重试
- 在提交结果不明确时查询 qBittorrent 状态后再决定是否重试
- 在最终失败后把关联剧集恢复为待搜索状态

## 刷流任务

- 绑定 PT 站点和 qBittorrent 下载器
- 使用 cron 定时执行或手动立即执行
- 按体积、做种数、促销类型、H&R 等条件选种
- 按做种时间、分享率、上传量、速度和活跃状态删种
- 查看任务统计、种子状态和流量快照

免费种和 H&R 信息优先使用 RSS 扩展属性；信息不足时，系统可从支持的站点详情页或 API 补充判定。

## 支持范围

- PT 搜索：NexusPHP API、NexusPHP Cookie HTML、M-Team
- 下载器：qBittorrent
- 影视来源：TMDB `movie` 和 `tv`
- 剧集编号：标准季集、中文季集、动画绝对集
- 视频质量：常见 480p 至 4K/8K、REMUX/BluRay/WEB、H.264/H.265/AV1 等发布名

当前未实现 scene/XEM 编号映射、每日剧日期编号、整季自动升级、更多下载器和通知渠道。搜索结果质量依赖站点返回信息和发布名称；无法可靠解析的资源不会被自动下载。

## 安全说明

云母当前不内置用户认证。Linux 命令行服务默认只监听 `127.0.0.1`。

如果监听 `0.0.0.0`、暴露到局域网或通过公网访问，必须自行限制网络访问，并放在带身份认证的反向代理后。CORS 限制不能替代身份认证。

TMDB Token、PT Cookie、API Key、Passkey 和下载器密码保存在本地 SQLite 中。请保护数据目录和备份，不要公开数据库文件。

接口不会在站点、下载器和媒体设置响应中回传已保存的明文凭据。更新配置时留空会保留原值，只有明确执行清除操作才会删除凭据。

## 数据目录

桌面应用使用系统应用数据目录，数据库位置如下：

| 平台 | 数据库 |
| --- | --- |
| Windows | `%APPDATA%\io.github.imythu.kirara\kirara.db` |
| macOS | `~/Library/Application Support/io.github.imythu.kirara/kirara.db` |

Linux 命令行服务默认路径：

```text
./data/kirara.db
```

使用 `--data-dir` 后：

```text
<data-dir>/kirara.db
```

主要数据包括：

- 全局系统设置
- PT 站点、账号数据缓存和下载器配置
- 刷流任务、种子记录和流量快照
- TMDB 设置和质量配置
- 影视订阅、订阅目标和搜索快照
- 媒体下载队列、重试和对账状态

升级或迁移前建议备份整个数据目录。

### 桌面版数据迁移

1. 停止旧程序，备份完整数据目录，包括数据库以及可能存在的 `-wal`、`-shm` 文件。
2. 如已打开新版桌面应用，先在托盘或菜单栏图标菜单中选择“退出”，确保后台服务已停止。
3. 将旧数据目录的完整内容复制到对应平台的应用数据目录。Windows 可在资源管理器地址栏输入 `%APPDATA%\io.github.imythu.kirara`；macOS 可在访达中使用“前往文件夹”打开 `~/Library/Application Support/io.github.imythu.kirara`。目录不存在时可自行创建。
4. 打开 Kirara，核对站点、下载器、订阅和任务状态。

旧数据库名为 `rflush.db` 时无需重命名。目标目录中已有 `kirara.db` 时会优先使用该文件；请先将新版生成的空数据目录移至备份位置，再复制旧数据，避免误打开空库。

## 开发

Web 服务后端：

```bash
cargo run
```

前端：

```bash
cd frontend
npm ci
npm run dev
```

前端开发服务器默认运行在 `http://127.0.0.1:5173`，并将 `/api` 请求代理到 `http://127.0.0.1:3000`。

本地构建命令行服务：

```bash
cd frontend
npm ci
npm run build

cd ..
cargo build --release
```

发布构建会将 `frontend/dist` 嵌入 Rust 可执行文件。

Windows / macOS 桌面开发需要安装 Rust、Node.js 和 [Tauri 2 的系统依赖](https://v2.tauri.app/start/prerequisites/)。在仓库根目录执行：

```bash
npm --prefix frontend ci
npm exec --prefix frontend -- tauri dev
```

桌面安装包构建：

```bash
# Windows
npm exec --prefix frontend -- tauri build --bundles nsis

# macOS
npm exec --prefix frontend -- tauri build --bundles dmg
```

Tauri 会自动运行前端构建。安装包输出到 `target/release/bundle/nsis` 或 `target/release/bundle/dmg`。后端公共库位于 `src/lib.rs`，命令行入口和 `src-tauri` 桌面入口共用同一套业务实现。

## 感谢与参考

本项目的自动追剧与资源搜索设计参考了以下开源项目：

- [Sonarr](https://github.com/Sonarr/Sonarr)：参考了剧集订阅、季集目标推进、质量配置、发布名称解析、候选筛选以及自动下载的整体产品思路。
- [pt_mate](https://github.com/JustLookAtNow/pt_mate)：参考了 PT 多站资源聚合搜索、NexusPHP 与 M-Team 站点适配、搜索结果归一化和资源获取流程。

云母没有直接照搬这些项目的实现，而是结合当前 Rust 后端、SQLite 状态管理、React 前端和已有 PT 站点配置体系重新设计并独立实现。感谢相关项目及其贡献者提供的思路和开源成果。
