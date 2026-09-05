# 从 rflush 迁移到云母 Kirara

后续开发、问题反馈和发布均移至 [imythu/kirara](https://github.com/imythu/kirara)。[旧仓库](https://github.com/imythu/rflush) 停止维护，保留历史代码和发布记录。本仓库保留迁移前的 Git 提交历史。

## 名称变化

| 项目 | 原名称 | 新名称 |
| --- | --- | --- |
| 仓库 | `imythu/rflush` | `imythu/kirara` |
| 程序 | `rflush` / `rflush.exe` | `kirara` / `kirara.exe` |
| 容器镜像 | `ghcr.io/imythu/rflush` | `ghcr.io/imythu/kirara` |
| 环境变量 | `RFLUSH_HOST`、`RFLUSH_PORT`、`RFLUSH_DATA_DIR` | `KIRARA_HOST`、`KIRARA_PORT`、`KIRARA_DATA_DIR` |
| 新安装数据库 | `rflush.db` | `kirara.db` |
| Rust 日志模块筛选 | `rflush=debug` | `kirara=debug` |

对外中文名继续使用“云母”。CLI 参数 `--host`、`--port`、`--data-dir` 和 API 路径保持不变。

## 已有安装

1. 停止旧程序或旧容器，避免两个服务同时调度相同的任务。
2. 备份**整个数据目录**，包含数据库以及可能存在的 `-wal`、`-shm` 文件。不要只复制正在运行中的 `.db` 文件。
3. 换用新仓库发布的二进制或镜像；在 systemd、Compose、启动脚本中更新程序路径、镜像名及三个环境变量。旧 `RFLUSH_*` 启动变量不再读取。
4. 将原数据目录原样挂载或传给 `kirara --data-dir`，然后启动新服务。
5. 核对站点、下载器、订阅及任务状态。确认使用正确的数据目录后再恢复日常自动化。

Kirara 优先打开 `kirara.db`；不存在该文件但存在 `rflush.db` 时，直接继续使用旧文件及其 SQLite 附属文件，**不会因改名而新建空库，也不会自动移动或复制数据库**。两种文件同时存在时以 `kirara.db` 为准，不合并数据。全新数据目录才会创建 `kirara.db`。已有安装无需手动改数据库文件名。

同一浏览器来源下，资源搜索的站点选择会兼容旧的本地存储键；保存时使用新的 `kirara` 键。若更换访问域名或端口，浏览器本地偏好不会跨来源转移，服务器数据库内容不受影响。

## Docker 示例

保持原来的数据卷或主机目录。下面的 `./data` 应替换为你实际使用的数据目录：

```bash
docker run --name kirara \
  -p 127.0.0.1:3000:3000 \
  -v "$(pwd)/data:/data" \
  ghcr.io/imythu/kirara:latest-beta
```

`latest-beta` 为自动构建预发布（Linux amd64）；正式版发布后使用 `latest`（Linux amd64 / arm64），也可使用具体版本标签。不要把旧仓库的版本号直接当作新镜像中已存在的标签。

## 开发者

```bash
git remote rename origin legacy
git remote add origin https://github.com/imythu/kirara.git
git fetch origin
git branch --set-upstream-to=origin/master master
```

新构建产物位于 `target/release/kirara`。前端包名为 `kirara-web`。旧仓库的 Issues、Pull Requests 和 Release 附件保留在旧地址，不会随 Git 历史自动复制。
