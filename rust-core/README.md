# IotaPanel Rust 核心（v0.4.0）

IotaPanel 的 Rust 微内核核心，与 Go 版**同一插件契约、同一数据格式**——`data/panel.json` 与 `etc/.env` 完全兼容，同一安装目录可在 Go/Rust 核心之间切换而不丢失登录态。

- 常驻内存 **~3MB**（Go 版约 8MB），二进制约 2.9MB
- 平台：Linux（arm64 / amd64）、Windows（amd64）；macOS 需在 Mac 或 CI 构建
- 纯 Rust：tokio + hyper（无 axum 路由依赖，请求处理全自控）

## 功能（v0.4.0）

**安全加固**
- 登录/登出（cookie 会话）、错误响应 401、CSRF Origin 校验（URL 解析比较，非字符串前缀）
- 插件名白名单（字母数字 `_-.`，拒绝路径穿越）；网关转发 **per-request 超时**（`PROXY_TIMEOUT`，默认 30s，插件挂起不再悬挂请求）
- 口令校验：PBKDF2-SHA256（60 万次迭代，与 Go 版同格式，旧 10 万次自动兼容）
- 会话令牌：base64url(JSON) + HMAC-SHA256（与 Go 版互认）
- 单账号单会话（新登录踢旧会话）；**API 会话**（`api:true`）不参与互踢（mcp-agent 兼容）

**初始化与账户**
- 初始化向导：`/setup` 创建管理员、自动生成 `JWT_SECRET` 写入 `etc/.env`
- 修改密码（校验旧密码、吊销其它会话）

**插件系统**
- manifest 契约（name/title/command/bind/keepalive/auth/menus）+ 环境变量注入
  （`PLUGIN_PORT`/`PLUGIN_BIND`/`PLUGIN_NAME`/`PANEL_HOME`/`IOTAPANEL_VERSION`）
- 端口池分配、`port-map.json` 写入、`/p/<name>/*` 反向代理网关
- 冷启动（按需拉起，6s 超时）、**空闲退出**（`IDLE_TIMEOUT`，默认 300s）
- **保活自愈**：`keepalive: true` 插件开机自动拉起、掉线自动恢复、不参与空闲退出
- **auth: none**：插件声明后 `/p/<name>/mcp` 免面板登录（插件自带 Bearer 鉴权，如 MCP Agent）

**面板 API（mcp-agent 写操作可用）**
- `GET /api/plugins`（列表 + 状态 + menus）
- `POST /api/plugins/<name>/start|stop|restart`
- `POST /api/plugins/<name>/set-keepalive {"keepalive": bool}`
- `GET /api/me`、`GET /api/overview`

**Web 管理前端（内嵌）**
- 侧边栏：概览 / 插件 / 账户 + 插件菜单自动注入（读 manifest menus）
- 插件管理：启停 / 重启 / 保活开关
- 修改密码表单；iframe 嵌入插件页面

## 兼容性实测（2026-08，官方 Go 插件）

| 插件 | 页面 | 说明 |
|---|---|---|
| hello | /p/hello/ | 环境变量注入正确 |
| file-manager | /p/file-manager/ | ✓ |
| resource-monitor | /p/resource-monitor/ | ✓ |
| terminal | /p/terminal/ | ✓ |
| https-front | /p/https-front/ | ✓ |
| mcp-agent | /p/mcp-agent/ | 免登录直连 `/mcp` + **写操作经 Rust 面板 API 实测可用** |

端到端验证：认证（登录/登出/401/CSRF/单会话/API 会话/改密）、初始化向导、插件启停/重启/保活、空闲退出、保活免疫、auth:none 豁免——全部通过。

## 构建

```bash
# 原生（当前架构）
cargo build --release

# 交叉编译 linux/amd64（需 gcc-x86-64-linux-gnu + libc6-dev-amd64-cross）
rustup target add x86_64-unknown-linux-gnu
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
  cargo build --release --target x86_64-unknown-linux-gnu

# 交叉编译 windows/amd64（需 gcc-mingw-w64-x86-64）
rustup target add x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build --release --target x86_64-pc-windows-gnu

# 一键打包（linux arm64/amd64 + windows，附 sha256）
./package.sh   # 产物在 dist/iotapanel-rust-0.4.0-*.tar.gz
```

## 运行

```bash
PANEL_HOME=/data/panel LISTEN_ADDR=127.0.0.1:8787 ./iotapanel-rust
# 首次访问自动跳转 /setup 初始化管理员；
# 之后 /login 登录 → 进入管理界面；插件从 /p/<name>/ 访问（需登录，除非 auth: none）。
# 空闲时长：IDLE_TIMEOUT=<秒>（默认 300）；转发超时：PROXY_TIMEOUT=<秒>（默认 30）
```

## 尚未实现（请勿用于生产迁移）

- 插件商店 / URL 安装（目前插件直接放入 `plugins/` 目录即用）
- 高级面板 API 面未全量移植（审计日志、统计图表等）
- macOS 产物需在 macOS 或 GitHub Actions macOS runner 构建
- 完整前端对 Go 版仍在收敛（差异均为增量，不影响核心契约）
