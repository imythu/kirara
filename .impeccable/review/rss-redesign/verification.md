# RSS 一体化订阅验证

- `npm --prefix frontend run check:rss`：通过。
- `npm --prefix frontend run build`：通过。
- `cargo test -q --lib db::rss::tests -- --test-threads=2`：18 项通过，包含整笔订阅创建/修改失败回滚、幂等重试和版本冲突。
- `cargo test -q --lib rss_download::tests -- --test-threads=2`：10 项通过，包含一体化预览无持久化副作用。
- `cargo test -q --test rss_downloader`：1 项通过。使用临时数据库、本地模拟 RSS 站点和 qBittorrent，经过真实 HTTP 新订阅接口、无副作用预览、无效下载器时整笔回滚、重复创建去重、首次基线、新资源只投递一次和重启恢复。未访问用户真实站点或投递用户下载器。初次运行中无效下载器的预期状态码从 422 校正为实际约定的 404 后通过。
- `frontend/tests/rss-subscriptions.browser.cjs`：1440/390px 通过新增、必填校验、预览、保存失败保留输入及请求 ID、统一编辑、放弃确认、保存为暂停、下载重试。
- 更新后的 `frontend/tests/rss.browser.cjs`：1440/390px 通过历史补下、匹配解释、高级规则管理、记录和下载恢复、URL 状态保留；原有桌面 IPC 和轮询节奏测试通过。浏览器使用明确标记的合成数据，非真实站点结果。
- OpenAPI YAML 解析及全部本地引用验证通过；`git diff --check` 通过。
- Impeccable detector：`[]`。独立检查唯一文档问题已由文档交接修正，`finish-verdict.md` 对该项判定 `ship`；不是额外一次全界面复审。
- `./dev.sh restart` 沿用 `/tmp/kirara-preview-31235`；随后 `./dev.sh status` 验证前后端运行、页面与 API 代理、任意 Host/Origin 均通过。预览 http://localhost:1234/#/rss。

生产后端仍有既有 dead-code 编译警告。未删除用户数据，未提交或推送 Git。
