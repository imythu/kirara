# 项目协作规则

## Web 界面启动

- 用户要求启动或预览 Web 界面时，监听地址统一使用 `0.0.0.0`，端口固定为 `1234`。
- `1234` 端口必须允许任意来源访问，不得限制为 localhost、本机 IP、指定域名或指定 Origin；通过任意域名、IP 或反向代理访问均应可用。
- Vite 的 `server` 和 `preview` 配置均必须保留 `host: "0.0.0.0"`、`allowedHosts: true` 和 `cors: true`，不得添加 Host 或 Origin 白名单。
- 使用 `strictPort: true`；端口被占用时先检查现有服务，不得静默切换到其他端口。
- 开发服务统一在项目根目录使用 `./dev.sh start|stop|restart|status` 管理，脚本同时管理前后端；可通过 `KIRARA_DATA_DIR` 指定数据目录，重启沿用该目录。
- 脚本启动前端时在项目根目录运行 `npm --prefix frontend run dev`；预览构建运行 `npm --prefix frontend run preview`。
- 启动后检查页面及 `/api` 代理可用，并使用自定义 Host 和跨域 Origin 验证任意来源访问未被拦截，再向用户提供 `http://<运行机器地址>:1234`。本机可使用 `http://localhost:1234`。
- 后端通过前端代理连接 `http://127.0.0.1:3000`。此规则针对 Web 界面；Tauri 桌面开发仍使用其显式配置的地址和端口。
