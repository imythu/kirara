# 项目协作规则

## Web 界面启动

- 用户要求启动或预览 Web 界面时，监听地址统一使用 `0.0.0.0`，端口固定为 `31234`。
- 允许任意来源访问：Vite 的 `allowedHosts: true` 和 `cors: true` 必须保留，开发服务及生产构建预览均适用。
- 使用 `strictPort: true`；端口被占用时先检查现有服务，不得静默切换到其他端口。
- 在项目根目录运行 `npm --prefix frontend run dev`；预览构建运行 `npm --prefix frontend run preview`。
- 启动后检查页面及 `/api` 代理可用，并向用户提供 `http://<运行机器地址>:31234`。本机可使用 `http://localhost:31234`。
- 后端通过前端代理连接 `http://127.0.0.1:3000`。此规则针对 Web 界面；Tauri 桌面开发仍使用其显式配置的地址和端口。
