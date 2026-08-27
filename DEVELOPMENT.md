# IotaPanel 开发文档（保姆级）

> 本文面向第一次接触本项目的开发者，从零开始：准备环境 → 跑起来 → 理解框架 → 开发插件 → 发布。
> 通篇使用可直接复制执行的命令与完整可运行的代码示例，并解释每一步**为什么这么做**。
> 若你只是想安装/使用面板，看 [README.md](README.md) 即可；本文假设你打算**改代码或写插件**。
>
> 📌 本项目即 **MicroPanel** 的更名版，现更名为 **IotaPanel**，遵循 Apache-2.0 许可证。

---

## 目录

1. [项目架构](#1-项目架构)
2. [目录结构](#2-目录结构)
3. [环境准备](#3-环境准备)
4. [跑起来](#4-跑起来)
5. [插件契约（核心规范）](#5-插件契约核心规范)
6. [从零写一个插件](#6-从零写一个插件)
7. [Rust 核心（rust-core/）](#7-rust-核心rust-core)
8. [测试](#8-测试)
9. [打包与发布（双核心双包）](#9-打包与发布双核心双包)
10. [常见问题](#10-常见问题)

---

## 1. 项目架构

**微内核**设计，两个核心（双核心共存）：

```
┌─────────────────────────────────────────────────┐
│            Web 管理前端（内嵌）                    │
│  ┌───────────┐    ┌──────────────────────────┐   │
│  │  认证/会话  │    │  /p/<插件名>/* 反向代理网关 │   │
│  │ PBKDF2    │    │  插件进程管理（启停/保活）    │   │
│  │ HMAC/CSRF │    └──────────┬───────────────┘   │
│  └───────────┘               │ PLUGIN_PORT 等环境变量
└──────────────────────────────┼──────────────────┘
                               ▼
        ┌─────────┐  ┌─────────┐  ┌──────────────┐
        │ 插件 A   │  │ 插件 B   │  │ 插件 C（任意语言）│
        │ (Go)    │  │ (Python) │  │ (Shell/Rust…) │
        └─────────┘  └─────────┘  └──────────────┘
```

- **Go 核心**（`cmd/`、`internal/`）：正式发布主力，功能最全（认证/会话/保活/前端/全部 API）。
- **Rust 核心**（`rust-core/`）：实验性重写（v0.4.0），**同一插件契约、同一数据格式**（`data/panel.json`、`etc/.env`），两个核心可互切而不丢登录态。
- **核心只做三件事**：认证（守住入口）、反向代理（`/p/<插件名>/*` 转发到插件端口）、插件进程管理（分配端口、冷启动、空闲退出、保活自愈）。
- **功能全部在插件里**：文件管理、终端、资源监控、HTTPS、MCP……都是独立进程。

**为什么这么设计？**
- 崩溃隔离：插件挂了不影响核心，核心挂了插件（保活）会被重新拉起。
- 语言无关：插件生态不被核心语言绑架；两个核心共享同一个插件生态。
- 资源省：核心常驻 Go ~8MB / Rust ~3MB，插件按需冷启动、空闲自动退出。

---

## 2. 目录结构

```
.
├── cmd/panel/            # Go 核心入口
├── internal/             # Go 核心内部包
│   ├── api/              #   鉴权中间件、CSRF、端口映射、插件网关、管理 API
│   ├── auth/             #   口令哈希（PBKDF2）、会话令牌、Cookie
│   ├── config/           #   配置、版本号
│   ├── db/               #   panel.json 持久化（用户/会话/插件记录）
│   ├── embed/            #   go:embed 资源（前端 + 内置插件，build.sh 生成）
│   └── plugins/          #   插件安装/加载/生命周期
├── plugins/              # 内置插件（每个独立 Go module）
│   ├── hello/  file-manager/  resource-monitor/
│   ├── terminal/  https-front/  mcp-agent/
├── rust-core/            # Rust 核心（v0.4.0）
│   └── README.md         #   Rust 核心独立文档
├── build.sh              # 编译 Go 核心 + 内嵌插件
├── package.sh            # 打包 Go 版（4 平台，附 sha256）
├── install.sh            # 安装/升级为 systemd 服务
├── internal/embed/web/   # Go 核心前端（index.html/app.js 等）
└── THIRD_PARTY_NOTICES.md
```

---

## 3. 环境准备

Go 核心需要 Go 1.27+；Rust 核心需要 Rust 1.85+（stable）。

```bash
# Go（Argo/其他平台去 go.dev 取对应包）
export GOPROXY=https://goproxy.cn,direct
export GODEBUG=netdns=go+4     # 纯 IPv4 DNS，避免部分网络环境解析失败

# Rust
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
```

---

## 4. 跑起来

```bash
# 编译 Go 核心（含内嵌插件，首次较慢）
./build.sh

# 开发运行（8787 端口）
PANEL_HOME=/tmp/mp-dev LISTEN_ADDR=127.0.0.1:8787 ./bin/panel

# 浏览器打开 http://127.0.0.1:8787 → 初始化向导创建管理员 → 登录
```

Rust 核心（原生直跑）：

```bash
cd rust-core && cargo build --release
PANEL_HOME=/tmp/mp-dev LISTEN_ADDR=127.0.0.1:8788 ./target/release/iotapanel-rust
```

**数据互通**：两个核心读写同一 `PANEL_HOME/data/panel.json` 与 `etc/.env`，登录态、用户、会话互相识别。

---

## 5. 插件契约（核心规范）

插件是**独立 OS 进程**：核心分配端口并注入环境变量，插件在指定端口起 HTTP 服务，
通过 `/p/<插件名>/*` 被面板网关反向代理。

### 5.1 目录与 manifest

```
插件名/                  # 目录名 = 插件名（全小写、字母数字_-）
├── manifest.yaml
├── bin/                 # 可执行文件（任意语言）
└── web/                 # 静态资源（可由插件自行提供）
```

`manifest.yaml` 最小示例（完整字段见 5.2）：

```yaml
name: hello            # 插件名（必须与目录名一致）
title: Hello
version: 0.1.0
command: bin/start.sh  # 相对插件目录的启动命令
bind: 127.0.0.1        # 监听地址
menus:
  - title: Hello
    icon: 👋
    path: /
    section: tools
```

### 5.2 manifest 字段全表

| 字段 | 必填 | 说明 |
|---|---|---|
| `name` | ✅ | 插件名，与目录名一致 |
| `title` | | 显示名 |
| `version` | | 版本号 |
| `command` | ✅ | 启动命令（相对插件目录） |
| `bind` | | 监听地址（默认 127.0.0.1） |
| `keepalive` | | `true` = 保活：开机自动拉起、掉线自愈、不参与空闲退出（适合服务型插件） |
| `auth` | | `none` = `/p/<name>/mcp` 端点免面板登录（插件自带 Bearer 等强鉴权时才可用！） |
| `menus` | | 侧边栏菜单列表：`{title, icon, path, section}` |

### 5.3 环境变量

核心注入给插件进程：

| 变量 | 说明 |
|---|---|
| `PLUGIN_PORT` | 分配到的端口（如 19001），必须监听此端口 |
| `PLUGIN_BIND` | 监听地址（对应 manifest `bind`） |
| `PLUGIN_NAME` | 插件名 |
| `PANEL_HOME` | 面板安装目录 |
| `IOTAPANEL_VERSION` | 核心版本号 |

**为什么用环境变量而不是配置文件？** 端口是核心动态分配的，配置随进程而来，零落地状态。

### 5.4 生命周期

- **冷启动**：首次访问 `/p/<插件名>/...` 时核心拉起进程，等待端口就绪（默认 6s 超时）。
- **空闲退出**：非保活插件空闲超过 `IDLE_TIMEOUT`（默认 300s）被回收释放内存；再次访问自动拉起。
- **保活**：`keepalive: true` 的插件开机自动启动、掉线自动恢复、永远不被空闲回收。
- **优雅退出**：插件收到 SIGTERM 后应关停 HTTP 服务（示例见 hello 插件）。

### 5.5 网关与页面前端

- 页面路径：`/p/<插件名>/` 及任意子路径都转发到插件端口。
- 插件页通过 iframe 嵌入主界面，地址栏不跳转；无需处理面板登录（核心已鉴权）。
- **`auth: none` 特例**：仅 `/p/<插件名>/mcp` 对世界开放（插件自鉴权），其余路径仍需登录。

### 5.6 port-map.json

核心把运行中的插件写入 `<PANEL_HOME>/etc/port-map.json`：

```json
{"hello": {"port": 19001, "pid": 12345, "started_at": "..."}}
```

可用于诊断与集成。

---

## 6. 从零写一个插件

以 Shell 插件为例（任意语言同理：起 HTTP 服务即可）。

```bash
mkdir -p my-plugin/bin my-plugin/web
cat > my-plugin/manifest.yaml <<'EOF'
name: my-plugin
title: 我的插件
version: 0.1.0
command: bin/start.sh
bind: 127.0.0.1
menus:
  - title: 我的插件
    icon: 🧩
    path: /
    section: tools
EOF
cat > my-plugin/bin/start.sh <<'EOF'
#!/bin/sh
cd "$(dirname "$0")/../web"
exec python3 -m http.server "${PLUGIN_PORT:-19000}" --bind "${PLUGIN_BIND:-127.0.0.1}"
EOF
chmod +x my-plugin/bin/start.sh
echo '<h1>我的插件 OK</h1>' > my-plugin/web/index.html
```

安装（任选其一）：

```bash
# ① 直接放入插件目录（即装即用）
cp -r my-plugin <安装目录>/plugins/

# ② 打成 tar.gz 从 URL 安装（可选 sha256；顶层目录名 = 插件名）
tar -C . -czf my-plugin.tar.gz my-plugin
```

刷新面板 → 侧边栏出现「我的插件」→ 点击即以 iframe 打开 `/p/my-plugin/`。

> Go 插件示例参考 `plugins/hello/`：`//go:embed web` 内嵌前端、监听 `$PLUGIN_PORT`、SIGTERM 优雅退出。

---

## 7. Rust 核心（rust-core/）

Rust 核心（v0.4.0）是实验性重写——**打底与验证架构用，不代表全部迁移**，Go 核心仍是生产主力。

- **一致性**：插件契约完全一致（manifest/环境变量/port-map/网关/`auth: none`）；数据与 Go 版互认（`panel.json` 用户/会话、`etc/.env` 的 `JWT_SECRET`）。
- **认证实现**：PBKDF2-SHA256（60 万次，salt 记 `600000:<hex>`）、会话令牌 `base64url(JSON).HMAC-SHA256`、cookie `mp_session`、单账号单会话（API 会话 `api:true` 不互踢）、CSRF Origin 校验、改密吊销其它会话。
- **面板 API**：`/api/plugins`（列表/启停/重启/保活）、`/api/me`、`/api/overview`——mcp-agent 写操作（plugin_action）可直接调用。
- **安全加固**：插件名白名单（字母数字 `_-.`、拒绝 `..`）、网关转发 per-request 超时（`PROXY_TIMEOUT`，默认 30s）、Origin 用 URL 解析比较。
- **管理前端**：内嵌于二进制（侧边栏/插件管理/账户页，与 Go 版同款暖米色主题）。

构建/打包/运行/缺口见 [rust-core/README.md](rust-core/README.md)。

---

## 8. 测试

```bash
# Go 单测（认证、存储、插件管理器、同源校验等）
go test ./... -count=1

# 插件级单测（如 mcp-agent 的工具权限门控与 MCP 分发）
go test ./plugins/mcp-agent/ -v -count=1
```

端到端冒烟清单（curl，可脚本化）：

```bash
# 1) 初始化 + 登录
curl -X POST http://127.0.0.1:8787/api/setup/start -d '{"username":"admin","password":"admin123"}'
curl -c /tmp/c.txt -X POST http://127.0.0.1:8787/api/login -d '{"username":"admin","password":"admin123"}'

# 2) 鉴权（未登录 → 401；登录后 → 200）
curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8787/p/file-manager/          # 401
curl -b /tmp/c.txt -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8787/p/file-manager/  # 200

# 3) 插件启停
curl -b /tmp/c.txt -X POST http://127.0.0.1:8787/api/plugins/mcp-agent/start

# 4) 空闲/保活：IDLE_TIMEOUT=6 启动核心，验证 6s 后普通插件被回收、保活插件存活
```

---

## 9. 打包与发布（双核心双包）

### 9.1 Go 核心（主力）

```bash
./build.sh          # 编译核心 + 内嵌插件（含全部插件页面修复）
./package.sh        # 产出 dist/iotapanel-<版本>-{linux-amd64,linux-arm64,windows-amd64,darwin-amd64}.tar.gz + .sha256
```

版本号在 `internal/config/config.go` 的 `Version`（如 `0.3.12`），README/文档中的版本号同步替换。
发布到 GitHub Releases：打 tag `v0.3.12` → 建 Release（附 release notes）→ 上传 8 个附件（4 包 + 4 sha256）。

### 9.2 Rust 核心（rust-core/）

```bash
cd rust-core && ./package.sh
# 产出 rust-core/dist/iotapanel-rust-0.4.0-{linux-amd64,linux-arm64,windows-amd64}.tar.gz + sha256
# macOS 需在 macOS 或 GitHub Actions macOS runner 构建（Apple SDK 闭源）
```

发布：打 tag `v0.4.0` → Release → 上传附件。两个核心独立打 tag、独立发布，互不影响。

---

## 10. 常见问题

**Q：插件可以共用吗？**
可以。插件是任意语言进程，只认契约（manifest/环境变量/网关/port-map），与核心语言无关——Go 编译的官方插件、Python/Shell 插件在 Go 与 Rust 核心下均可运行（仅受架构匹配限制）。

**Q：为什么面板占用这么小？**
核心只做认证/反代/进程管理，功能全部按需冷启动的插件承担；空闲插件会被回收。

**Q：改密/强制下线后网页提示未登录怎么办？**
这是正常行为：会话被吊销后自动回到登录页重新登录（v0.3.11 修复了此前 `/` 与 `/login` 之间 302 死循环的问题）。

**Q：`auth: none` 有风险吗？**
有——等于把 `/p/<name>/mcp` 开放到公网（绕过面板登录）。仅当插件自带强鉴权（如 Bearer 令牌）时才应使用；第三方插件不要轻易声明。

**Q：怎么升级？**
下载新版包 → 重跑 `./install.sh`（自动清理旧版服务名）→ 数据目录（`data/`、`etc/`、`plugins/`）保留不动。升级后如遇插件异常，先在插件页重装对应插件（manifest 可能随版本变化）。