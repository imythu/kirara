# WebDAV Cookie 同步验证

日期：2026-09-06。所有验证使用临时数据库或模拟 API，未读写用户站点凭据。

| 检查 | 结果 |
|---|---|
| `cargo test -p kirara webdav --lib` | 12 通过，含手动导入、接收服务和既有 WebDAV 路径测试 |
| `cargo test -p kirara --test server_lifecycle` | 5 通过 |
| `npm run build`（frontend） | 通过 |
| `frontend/tests/webdav-sync.browser.cjs` | 1440×1000、390×1000 通过，无页面异常或水平溢出 |
| `webdav@5.10.0` 端到端客户端 | Basic 认证、目录列表、PTD 推送、文件下载、后台自动建站、重复上传、删除均通过 |
| OpenAPI YAML | 解析通过，所有内部引用可解析 |
| `git diff --check` | 通过 |
| 全量 TypeScript 类型检查 | 有 13 项原有错误；对 HEAD 前端快照执行相同检查，归一化路径后的输出与本次完全一致 |

专项覆盖：Cookie 域名、Secure 和有效期过滤；归档大小与结构校验；已有配置和 Passkey 保留；旧数据不覆盖新凭据；未变化不写站点记录；上传去重；同名新版本保持旧任务数据；重启后继续处理；数据库异常回滚站点变更及任务进度；队列上限；密码一次性返回及重置失效；管理 API 与 WebDAV 共用端口、启停配置即时生效。

浏览器覆盖：首次加载失败后的重新加载、保存失败保留输入、轮询不覆盖编辑、密码生成与不回传、同步记录、记录内重试失败提示、再次重试、原有备份面板。独立界面复核提出的两项问题均已修复并获得 `ship` verdict，详见 `.impeccable/review/webdav-sync/review.md`。

TypeScript 原有错误涉及 Vite/Node 类型配置、ImportMeta.env、刷流页可空参数及统计页面组件类型；没有新增错误。本功能的生产打包和实际浏览器运行通过，但不能将全仓类型检查宣称为通过。

本次未执行真实 PT 站点登录测试、PTD 浏览器扩展端到端操作或各操作系统的打包发布。协议端到端使用 PTD 所依赖的同版本 WebDAV 客户端、真实云母进程及合成的 PTD 备份数据。

同端口调整：已移除独立监听器及地址/端口配置；兼容读取旧配置。专项测试验证同一端口管理与接收、关闭接收即时拒绝请求；真实客户端联调还检查 Web 首页可用及未认证 OPTIONS 不被 CORS 层放行。桌面 IPC 模式明确提示使用 Web 服务版。

手动导入调整：`frontend/tests/ptd-import.browser.cjs` 在 1440×1000 和 390×1000 下验证直接入口、空文件拦截、错误保留文件后重试、传输内容与原文件一致、结果及跳过原因展示，无页面异常或水平溢出。后端测试覆盖关闭 WebDAV 后仍可手动导入、ZIP/JSON 共用匹配、重复未变化、更新/跳过策略、错误格式拒绝、结果不包含 Cookie 明文，以及数据库异常回滚站点和任务记录。UI 截图位于 `.impeccable/review/ptd-import/`。

最终真实进程联调通过：同一 Web 端口上的 WebDAV 推送和手动 POST 导入均可更新同一站点；手动文件采用超过 2 MiB 的合法 JSON，验证专用请求大小限制生效。独立界面复核结论为 `ship`，见 `.impeccable/review/ptd-import/review.md`。
