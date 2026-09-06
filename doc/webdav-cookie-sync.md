# WebDAV Cookie 自动同步

站点管理 → **备份与同步 → Cookie 自动同步**。开启接收服务并保存，复制本次生成的密码。将页面上的接收地址、用户名和密码填入 PTD 的 WebDAV 备份配置，关闭 Digest，勾选 Cookie，并设置自动备份周期。

接收服务与现有 Web 服务共用监听地址和端口，路径为 `/dav/ptd/`。Linux 和 Docker 的 Web 服务均适用；桌面版通过 Unix Socket / Named Pipe 通信，没有对外 Web 端口，因此本功能需使用 Web 服务版。启用、关闭和密码变更在配置保存后生效，后台每秒检查同步队列。旧配置中的独立监听地址和端口不再使用，原有账号、密码及同步记录保留。

## 配置与部署

- 直接复制页面显示的 WebDAV 地址。例如 Web 页面为 `http://127.0.0.1:3000`，则填写 `http://127.0.0.1:3000/dav/ptd/`；自定义端口和 HTTPS 同样沿用当前 Web 地址。
- 从其他设备连接时，使用该设备可访问的云母 Web 地址，监听范围沿用主 Web 服务的启动配置。
- Docker 沿用现有 Web 端口映射，无需额外发布接收端口。
- 远程访问使用 HTTPS 反向代理，转发完整 `/dav/ptd/` 路径及 Authorization、Depth 请求头，允许 PROPFIND、PUT、GET、HEAD、DELETE、OPTIONS。Basic 认证由接收服务验证。
- 一个接收配置用于一个 PTD 来源。多个设备轮流推送同一账户的数据会按接收顺序和可用的来源时间处理，无法判断哪个浏览器的登录状态更可信。

密码由服务端生成 32 个随机字节并显示为 64 位十六进制文本，只存储其 SHA-256 摘要；校验使用常量时间比较。这不是供用户设置低强度密码的接口。生成密码的保存响应只显示一次明文，GET 配置不会返回明文或摘要。生成新密码会立即替换旧密码，同时保存表单设置。

## 同步行为

默认更新已有站点 Cookie，并自动添加有适配预设的新站点；两项均可在接收设置中调整。更新只修改认证配置中的 Cookie 和站点更新时间，保留站点 ID、名称、地址、代理、自定义请求头、Passkey 及关联任务。更新时间变化使账户身份缓存重新验证。内容未变化时不写站点记录。

PTD 站点 ID 用于匹配，Cookie 必须适用于现有站点的实际域名。相同站点出现多条现有记录时跳过，避免修改错误账户；不支持的站点和不使用 Cookie 的认证方式也跳过并显示原因。数据中缺少某站点不会删除本地站点。文件内部重复及冲突在规范化函数中处理，不弹出额外选择流程。

接收端兼容 PTD 原生未加密备份及直接 PUT 的域名到浏览器 Cookie 数组 JSON。手动导入和自动同步共用相同解析与站点更新逻辑。解析限制单次请求和单个展开文件均为 8 MiB，最多 32 个归档成员、500 个域名、10,000 条 Cookie；校验 manifest 和 Cookie 文件摘要。Cookie 过滤有效期、域名、hostOnly、Secure、根路径及头部字符；不将不同域名的登录凭据混合。加密备份返回明确错误，需在 PTD 中关闭备份加密；网络传输可通过 HTTPS 保护。

## 接收与持久化

`PUT` 完整接收并校验后，将不可变数据版本与待处理任务在一个 SQLite 事务中持久化，再返回 `201`（新文件）或 `204`（覆盖或重复传输）。因此 PTD 的上传成功只代表已接收，站点更新结果在同步记录中查看。

相同路径与内容的重传复用现有任务。覆盖同名文件会建立新版本，已排队任务保留其原始数据；后台每秒处理一个待执行任务。站点更新、每站已应用的来源时间及任务结果在一个事务中提交，进程中断不会留下半完成的同步。启动后继续读取未提交任务，不依赖内存队列。

有来源生成时间时，较旧的数据不会覆盖该站点已经应用的较新数据；无来源时间的 JSON 只能按接收顺序处理。旧任务重试还会检查已应用任务版本，避免恢复后的旧任务回写新 Cookie。生成时间超过服务端时钟五分钟的备份会被拒绝，需校正发送设备时间。

临时数据库错误回滚整个任务，按 1、5、15 分钟重试，仍失败则标记失败。仍保留数据的失败任务可以在页面重新同步；数据被清理后需从 PTD 重新推送。格式错误在 PUT 阶段返回 `422`，不会加入后台队列。

目录保留最近 5 个文件路径，已完成任务保留最近 100 条记录；尚未完成的任务数据不会提前清理。同名文件旧版本处理完成后可清理。存储中的上传内容总量上限为 64 MiB，待处理任务最多 100 条，超限返回 `507`；上传并发上限为 4，单次上传超时为 30 秒。

## 接口

接收端只开放 `/dav/ptd/` 下的固定根目录和单层 ASCII `.zip` / `.json` 文件名，没有文件系统路径映射；同一端口的管理 API 仍位于 `/api/` 下。

| 方法 | 行为 |
|---|---|
| OPTIONS | 返回支持的方法 |
| PROPFIND | 支持 Depth 0/1，返回目录与文件属性的 XML 多状态响应 |
| PUT | 完整保存后创建同步任务 |
| GET / HEAD | 原样读取仍保留的文件或查询长度、ETag |
| DELETE | 删除接收文件引用，已经排队的同步任务继续运行，不删除站点 |

这是面向 PTD 的有限 WebDAV 兼容接口，不声明完整 WebDAV compliance class，不实现锁、移动和任意目录管理。所有操作均要求独立的 Basic 接收凭据。管理接口沿用项目原有管理访问边界，不接受接收密码作为管理认证。

管理 API 见 `doc/openapi.yaml`：`GET/PUT /api/sites/webdav-sync`、`GET /api/sites/webdav-sync/runs`、`POST /api/sites/webdav-sync/runs/{id}/retry`。

## 验证

```sh
cargo test -p kirara webdav --lib
cargo test -p kirara --test server_lifecycle
cd frontend
npm run build
```

浏览器回归使用 `frontend/tests/webdav-sync.browser.cjs`，所有 API 请求均被模拟，覆盖桌面与手机布局、加载和保存失败恢复、轮询不覆盖编辑内容、一次性密码、记录重试和原有备份面板。设置 `PLAYWRIGHT_MODULE` 指向测试用 Playwright，启动 Vite 后运行；截图默认写入 `/tmp/kirara-webdav-ui`。

`frontend/tests/webdav-client.mjs` 使用 PTD 同版本的 `webdav@5.10.0` 客户端启动真实云母进程，在隔离临时数据库中验证推送、目录列表、下载、自动建站、重复请求和删除。先 `cargo build -p kirara`，再运行该脚本；通过 `WEBDAV_MODULE` 指向测试安装的 `webdav/dist/node/index.js`。凭据与站点数据均为合成测试数据，测试结束后停止进程并清理数据库。

## 手动导入 PTD 配置

站点管理页面点击 **导入 PTD 配置**，或在「备份与同步」中切换到同名面板。Web 服务版和桌面版均可使用，无需开启 WebDAV。选择未加密 PTD ZIP 或其中的 `cookies.json`（不超过 8 MiB），选择已有站点更新/跳过以及是否自动添加新站点，点击「开始导入」。结果立即显示，站点列表自动刷新；本次操作采用一个事务，数据库失败则整体回滚。

PTD 导出步骤：

1. 在安装 PTD 的浏览器中登录需要导入的 PT 站点。
2. PTD「常规设置 → 备份恢复」，将「备份文件加密、解密密钥」留空；保留原密钥供恢复旧备份，导出后可恢复原设置。
3. 「参数备份与恢复 → 本地导出」，勾选「站点 Cookies（cookies）」，点击导出。其他项目无需勾选。
4. 选择下载的 `PTD_backup_*.zip` 导入，或解压后选择 `cookies.json`。

导出项核对自 [PTD 本地导出界面](https://github.com/pt-plugins/PT-depiler/blob/master/src/entries/options/views/Settings/SetBackup/LocalExportConfirmDialog.vue)、[中文界面文案](https://github.com/pt-plugins/PT-depiler/blob/master/src/locales/zh_CN.json) 和 [本地备份生成逻辑](https://github.com/pt-plugins/PT-depiler/blob/master/src/entries/offscreen/utils/backup.ts)。不同版本的入口名称可能略有变化。

此功能从 Cookie 数据识别站点并设置登录凭据，不恢复下载器、PTD 插件偏好或任务。导入选项仅作用于本次手动操作，不修改自动同步配置。手动结果也记录在同步历史中，不将上传文件发布到 WebDAV 目录。重复导入按共同匹配规则处理，旧来源时间仍受保护。

管理 API：`POST /api/sites/ptd-import`，JSON 字段为 `content_base64`（文件内容的 Base64）、`existing_policy`（update/skip）和 `auto_create`；返回 created/updated/unchanged/skipped/details。使用 JSON 传输兼容桌面版现有 IPC 通道，文件解析及明文 Cookie 不出现在结果中。
