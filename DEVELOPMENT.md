# IotaPanel 开发文档（保姆级）

> 本文面向第一次接触本项目的开发者，从零开始：准备环境 → 跑起来 → 理解框架 → 开发插件 → 发布。
> 通篇使用可直接复制执行的命令与完整可运行的代码示例，并解释每一步**为什么这么做**。
> 若你只是想安装/使用面板，看 [README.md](README.md) 即可；本文假设你打算**改代码或写插件**。
>
> 📌 本项目即 **MicroPanel** 的更名版：原名 MicroPanel（[github.com/plainfate/IotaPanel](https://github.com/plainfate/IotaPanel)），现更名为 **IotaPanel**，遵循 GPL-3.0 许可证。

---

## 目录

1. [环境准备](#1-环境准备)
2. [五分钟跑起来](#2-五分钟跑起来)
3. [框架总览](#3-框架总览)
4. [核心模块详解](#4-核心模块详解)
5. [插件开发（保姆级教程）](#5-插件开发保姆级教程)
6. [核心开发指南（改框架）](#6-核心开发指南改框架)
7. [常见问题 FAQ](#7-常见问题-faq)

---

## 1. 环境准备

| 依赖 | 版本 | 说明 |
|---|---|---|
| Go | **1.25+** | `go.mod` 中 `go 1.25.0`；低于 1.25 编译不过 |
| 操作系统 | Linux 优先 | 核心 + 全部官方插件在 Linux 上功能完整；Windows/macOS 只有简化版 |
| gzip | 任意 | `build.sh` 压缩内嵌插件用（一般系统自带） |
| git / curl | 任意 | 版本管理与下载依赖 |

检查环境：

```bash
go version        # 需要 >= go1.25
gzip --version
```

> 国内网络拉取 Go 依赖慢/失败时，设置代理后重试：
> ```bash
> export GOPROXY=https://goproxy.cn,direct
> ```

---

## 2. 五分钟跑起来

```bash
git clone git@github.com:plainfate/IotaPanel.git
cd IotaPanel

# 1. 构建（编译 4 个官方插件并内嵌 + 编译核心 → bin/panel）
./build.sh

# 2. 运行开发版（数据放到 /tmp/mp-dev，不污染系统）
PANEL_HOME=/tmp/mp-dev ./bin/panel

# 3. 浏览器打开 http://127.0.0.1:8787 → 首次进入初始化向导
#    建管理员账号 + 勾选基础插件即可

# 4. 跑测试（另开终端）
go test ./...
```

**关键点**：`PANEL_HOME` 是面板的"家"——所有数据（数据库、插件、日志、配置）都在这个目录下，删掉它等于重置面板。开发时用 `/tmp/xxx` 最省心。

---

## 3. 框架总览

### 3.1 一张图看懂架构

```text
浏览器
  │  HTTP(S) 请求
  ▼
┌─────────────────────── 面板核心（单个 Go 二进制，常驻约 8MB）───────────────────────┐
│  HTTP 服务器 :8787                                                               │
│  ├─ 静态前端（内嵌 HTML/CSS/JS，登录页/主界面/向导）                               │
│  ├─ REST API（/api/*：认证、插件管理、设置、账户…）                                │
│  └─ 反向代理网关（/p/<插件名>/*：转发到插件进程，带登录校验）                       │
│  组件：config 配置 · db 存储 · auth 认证 · plugins 进程管理 · gateway 网关          │
└───────────────────────────────────────────────────────────────────────────────────┘
  │  按需冷启动：注入环境变量 + 分配端口
  ▼
┌─ 插件进程 A（任意语言，独立进程，独立端口）─┐  ┌─ 插件进程 B ─┐
│ 如 file-manager / terminal / 你自己写的插件   │  │   …          │
└───────────────────────────────────────────┘  └──────────────┘
```

**设计哲学（记住这三点，看代码就不迷路）**：

1. **核心只做三件事**：用户认证、反向代理网关、插件进程管理。不含任何"运维功能"（文件管理、监控……都是插件）。
2. **插件 = 独立进程**：与核心之间只通过「环境变量 + HTTP + 端口映射」通信，不共享内存、不做进程内调用。插件崩了核心没事，核心重启也不杀保活插件。
3. **按需启动**：开机只有核心在跑；点菜单才拉起插件，空闲自动退出。

### 3.2 目录结构

```text
IotaPanel/
├── cmd/panel/                # 核心入口（main.go + cli.go）
├── internal/
│   ├── config/               # 配置：.env / 环境变量解析
│   ├── db/                   # 存储：轻量 JSON（users/plugins/sessions/settings）
│   ├── auth/                 # 认证：PBKDF2 口令哈希 + HMAC 会话令牌
│   ├── plugins/              # 插件管理器：生命周期 + 安装/卸载（最重要的模块）
│   ├── gateway/              # 反向代理网关（/p/<name>/*）
│   ├── api/                  # REST API 与页面路由（所有 HTTP 入口）
│   └── embed/                # 内嵌资源：
│       ├── web/              #   前端（纯 HTML/CSS/JS，编译期内嵌）
│       └── plugins/          #   官方插件包（build.sh 生成，gzip 压缩后内嵌）
├── plugins/                  # 官方插件源码（file-manager / resource-monitor / hello / terminal）
├── build.sh                  # 构建脚本（插件内嵌 + 核心编译）
├── package.sh                # 多平台打包脚本（产出 dist/*.tar.gz + .sha256）
├── install.sh                # 一键安装脚本（部署 + systemd + panel 命令）
└── .github/workflows/        # CI：build + vet + test
```

依赖极少：核心只有 `gopkg.in/yaml.v3`（manifest 解析），终端插件额外用 `creack/pty` 和 `gorilla/websocket`。

---

## 4. 核心模块详解

### 4.1 入口 `cmd/panel/main.go` —— 启动流程

`main()` 做的事按顺序：

1. **解析命令行**：`panel version/start/stop/restart/status/log/uninstall/help` 走 `runCLI`；`panel serve`（或没有参数）才真正启动服务。**未知参数会报错退出**（不要当成服务启动）。
2. **设置内存上限**：`GOMEMLIMIT=48MB`（可用环境变量覆盖），防止突发请求撑大常驻内存。
3. **加载配置** `config.Load()`：确定 `PANEL_HOME` → 读 `.env` → 组装配置（监听地址、JWT 密钥、空闲超时、端口池）。
4. **写运行标记** `/tmp/iotapanel-home`（0600 权限），供非 systemd 下 `panel start` 恢复安装目录。
5. **打开数据库** `db.Open()`（JSON 原子写盘，损坏自动回退 `.bak`）。
6. **扫描插件目录** `syncPluginsFromDir()`：`<home>/plugins/` 下手动放入的插件目录自动登记（拷贝即安装）。
7. **创建插件管理器** `plugins.NewManager()` + `Load()`：读取 `port-map.json`，认领仍存活的插件进程（核心重启不杀保活插件的关键）。
8. **创建 HTTP 服务** `api.NewServer()`，启动监听。

### 4.2 配置 `internal/config/config.go`

配置来源：**环境变量 > `.env` 文件**（`<home>/etc/.env`，环境变量优先）。

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `PANEL_HOME` | 自动推导或 `/data/panel` | 安装目录。解析顺序：环境变量 → `.env` → 二进制位置（`<dir>/bin/panel` → 父目录）→ 兜底 `/data/panel` |
| `LISTEN_ADDR` | `:8787` | `:8787` 全部网卡双栈；`0.0.0.0:8787` 仅 IPv4；`127.0.0.1:8787` 仅本机 |
| `JWT_SECRET` | 自动生成并写入 `.env` | 会话签名密钥，务必保密；丢失后所有会话失效 |
| `IDLE_TIMEOUT` | `5m` | 插件空闲退出时间（Go duration 格式，如 `5m`/`90s`） |
| `PORT_START` / `PORT_END` | `19000` / `19999` | 插件端口池 |
| `PANEL_TRUST_PROXY` | 关 | 面板部署在**受信反代**之后才设 `1`：信任 `X-Forwarded-*` 头（CSRF 校验、cookie Secure、网关协议透传）。直连模式忽略这些头，防伪造 |

> ⚠️ 公网部署必须前置 HTTPS 反向代理（Nginx/Caddy），并设 `PANEL_TRUST_PROXY=1`。

### 4.3 存储 `internal/db/db.go`

- 单一 JSON 文件 `data/panel.json`，结构：`users / plugins / sessions / settings`。
- **原子写盘**：先写 `panel.json.tmp` → rename 覆盖 → 旧文件保留为 `.bak`。主文件损坏/缺失时 `Open` 自动回退 `.bak`；残留 `.tmp` 启动时清理。
- 所有写操作即时落盘（写穿式），无需手动保存。
- 会话令牌**只存 SHA-256 指纹**，不存明文。

### 4.4 认证 `internal/auth/auth.go` + `internal/api/server.go` 的 auth 中间件

- **口令哈希**：PBKDF2-SHA256，**60 万次迭代**，每用户随机 16 字节盐。盐格式 `"600000:<hex>"` 携带迭代次数；旧版 10 万次哈希登录后**自动升级**（`NeedsRehash` + 登录时重写）。
- **会话令牌**：`base64url(JSON{uid, u, exp, j}) . HMAC-SHA256签名`，写入 cookie `mp_session`（HttpOnly + SameSite=Lax，HTTPS 下 Secure）。
- **中间件流程**（`auth()`）：
  1. 读 cookie；
  2. 验签 + 校验过期（`ParseToken`）；
  3. 按令牌指纹查数据库会话 → 必须存在且未被吊销（支持强制下线/踢人）。
- **单账号单会话**：新登录立即吊销该账号其他会话。
- **登录保护**：失败锁定（默认 5 次 / 15 分钟，`/api/security` 可调）、登录页可选「记住我」（30 天）。

### 4.5 插件管理器 `internal/plugins/manager.go` —— 最核心的模块

插件生命周期状态机：

```text
未运行 ──Start()──▶ 启动中（分配端口→注入环境变量→exec→等端口就绪 ≤6s）
                        │ 成功
                        ▼
                   运行中 ──Touch() 重置空闲计时──▶ 继续运行
                        │ 空闲超时（非保活）           │ 保活
                        ▼                             ▼
                   idle 退出（回收）             常驻（核心重启后由 port-map 认领复用）
```

关键机制（阅读源码时的路线图）：

| 机制 | 位置 | 说明 |
|---|---|---|
| 冷启动 | `Start()` | 读 manifest → `allocPortLocked(bind)` 分配端口 → `exec` 插件进程（注入环境变量）→ `waitPort` 等端口就绪（6s 超时）→ 写 `port-map.json` |
| 空闲退出 | `armIdleLocked` | `time.AfterFunc(IDLE_TIMEOUT)` **事件驱动**，无常驻轮询协程；每次请求 `Touch()` 重置计时器 |
| 保活 | `ApplyKeepalive` / `Load()` | 保活插件跳过空闲计时；核心 SIGTERM 时不杀；重启后 `Load()` 扫描 `port-map.json`，端口仍被占用则**认领复用同 PID**，不重启进程 |
| 停止 | `Stop()` | 删除运行时条目 → `killProc`：SIGTERM 优雅退出，等 3 秒再 SIGKILL；**发信号前校验进程启动节拍**（防 PID 被系统复用后误杀无辜进程） |
| 崩溃清理 | `Start()` 内的 goroutine | 进程退出（`cmd.Wait` 返回）立即从运行表删除条目 → 下次请求自动重新拉起 |
| 日志 | `Start()` | 插件输出写入 `<home>/logs/plugins/<name>.log`，超 20MB 启动时轮转保留 `.1` |

### 4.6 网关 `internal/gateway/proxy.go`

- 路由：`/p/<插件名>/<路径>`（**必须登录**）。
- 行为：插件未运行则先 `Start()` 冷启动（约 0.2-1 秒）→ `Touch()` 重置空闲计时 → `httputil.ReverseProxy` 转发到 `http://<插件bind>:<端口>`。
- 注入请求头：`X-Forwarded-Proto`（受信反代模式下透传）、`X-Forwarded-Host`（原始 Host）、`X-Panel-Plugin`（插件名）。
- 支持 WebSocket 升级（终端插件依赖），并实现了 `http.Flusher` 透传（SSE/流式响应）。

### 4.7 API 层 `internal/api/`

**中间件栈**（外层→内层）：

```text
logRequests（访问日志） → securityHeaders（安全响应头） → csrfCheck（Origin 校验） → 路由（auth 认证）
```

| 中间件 | 作用 |
|---|---|
| `logRequests` | 记录 method/path/status/耗时 |
| `securityHeaders` | `X-Frame-Options: SAMEORIGIN`、`X-Content-Type-Options: nosniff`、HTTPS 下 HSTS |
| `csrfCheck` | POST/PUT/DELETE 校验 `Origin` 头：缺失（非浏览器）放行；存在但与面板 Host 不同源 → 403 |
| `auth` | 校验登录会话（见 4.4），写入会话到请求 context |

**路由速查**（完整列表见 `server.go Handler()`）：

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/login` `/api/logout` | 登录 / 退出（logout 会按令牌指纹吊销服务端会话） |
| GET | `/api/status` `/api/me` | 状态 / 当前用户 |
| GET/POST | `/api/setup/state` `/start` `/status` | 初始化向导（未初始化时开放） |
| GET/POST/DELETE | `/api/plugins` `/api/plugins/{name}/start\|stop\|restart\|keepalive\|log` | 插件管理 |
| POST | `/api/store/{name}/install` `/api/store/install-url` | 安装官方 / URL 插件 |
| GET/PUT | `/api/settings` `/api/security` | 设置 / 安全策略 |
| GET/POST | `/api/account` `/api/account/password` `/api/account/username` `/api/account/sessions*` | 账户与会话 |
| POST | `/api/system/restart` | 重启面板 |
| GET | `/p/{name}/*` | 插件网关（登录） |
| GET | `/` `/login` `/setup` `/css/*` `/js/*` | 前端页面 |

### 4.8 前端 `internal/embed/web/`

- **纯原生 HTML/CSS/JS，无构建步骤**——编译期内嵌进二进制，所以保持"单一自包含文件"。
- 页面：`login.html`（登录）、`setup.html`（初始化向导）、`index.html`（主界面）。
- `js/app.js`：主逻辑（路由、侧边栏、插件页、设置页）；`js/i18n.js`：18 种语言。
- **安全约定**：所有进入 `innerHTML` 的动态文本必须过 `esc()` 转义（防止存储型 XSS——插件元信息是外部输入）。
- 主界面前端调用 API 用**绝对路径**（`/api/...`）；**插件自己的前端页面必须用相对路径**（见 5.6）。

---

## 5. 插件开发（保姆级教程）

### 5.1 插件是什么 —— 三条铁律

**插件 = 一个目录 + `manifest.yaml`**，目录放到面板的 `plugins/` 下即被识别。你的程序只要满足：

1. **监听 `$PLUGIN_PORT` 端口**（HTTP），提供页面或 API；
2. **收到 SIGTERM 优雅退出**（面板停止/重启时调用）；
3. **默认只监听 `127.0.0.1`**（`manifest.bind`，外部流量统一走面板网关，更安全）。

除此之外**不限语言**：Go、Rust、Python、Node.js、Shell……什么都能写。核心不关心你用什么语言，只负责"分配端口 + 拉起进程 + 反向代理"。

### 5.2 `manifest.yaml` 完整参考

```yaml
name: my-plugin        # 必填。插件唯一标识，必须与目录名一致
title: 我的插件          # 侧边栏显示名（缺省 = name）
version: 0.1.0
author: 你的名字
description: 一句话描述
language: go           # 语言标记（仅展示用）
command: bin/my-plugin # 必填。入口可执行文件/脚本，相对插件目录
args: []               # 可选。传给 command 的参数
bind: 127.0.0.1        # 监听地址（默认 127.0.0.1）。对外服务才改 0.0.0.0
keepalive: false       # 可选。安装时默认开启保活（进程常驻）
menus:                 # 可选。注入侧边栏的菜单（可多个）
  - title: 我的插件
    icon: 🧩
    path: /            # 插件页面内的路径（iframe 指向 /p/<插件名>/<path>）
    section: tools     # 分组标记（目前侧边栏按插件分组渲染，该字段为扩展预留）
```

校验规则（`LoadManifest`）：`name` 与 `command` 必填，缺一报错、插件不被识别。

### 5.3 核心注入的环境变量

| 变量 | 说明 |
|---|---|
| `PLUGIN_PORT` | 分配的端口，**必须监听它** |
| `PLUGIN_BIND` | 监听地址（= manifest.bind） |
| `PLUGIN_NAME` | 插件名 |
| `PANEL_HOME` | 面板安装目录 |
| `IOTAPANEL_VERSION` | 核心版本号 |

### 5.4 教程一：Go 插件（逐行解读官方 hello）

官方 `plugins/hello/main.go` 是最小可运行示例，核心只有三块：

```go
func main() {
	port := os.Getenv("PLUGIN_PORT")      // ① 读面板注入的端口
	if port == "" {
		port = "19003"                     // 手动运行时兜底端口
	}
	addr := "127.0.0.1:" + port

	mux := http.NewServeMux()
	mux.HandleFunc("GET /", handleIndex)          // ② 页面
	mux.HandleFunc("GET /api/info", handleInfo)   //     API

	server := &http.Server{Addr: addr, Handler: mux, ReadHeaderTimeout: 10 * time.Second}
	go func() {                                    // ③ 优雅退出
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		server.Close()
	}()
	server.ListenAndServe()
}
```

三步即契约：**读端口 → 起 HTTP 服务 → 处理 SIGTERM**。页面/API 逻辑就是普通 Go HTTP handler。

### 5.5 教程二：Shell/Python 插件（零编译）

不需要 Go 也能写插件。创建一个目录 `my-shell-plugin/`：

```bash
my-shell-plugin/
├── manifest.yaml
├── bin/start.sh
└── web/index.html
```

`manifest.yaml`：

```yaml
name: my-shell-plugin
title: Shell 示例
version: 0.1.0
author: 你
description: 用 Shell + Python3 写的插件
language: shell
command: bin/start.sh
bind: 127.0.0.1
menus:
  - title: Shell 示例
    icon: 🐚
    path: /
    section: tools
```

`bin/start.sh`（用 Python3 起一个静态文件服务，展示 `$PLUGIN_PORT`）：

```bash
#!/usr/bin/env bash
# 监听面板分配的端口，服务 web/ 目录
cd "$(dirname "$0")/../web"
exec python3 -m http.server "${PLUGIN_PORT:-19090}" --bind 127.0.0.1
```

`web/index.html`：

```html
<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>Shell 插件</title></head>
<body><h1>Shell 插件运行中</h1></body></html>
```

安装方式三选一（见 5.9），装好后点侧边栏菜单即可访问 `/p/my-shell-plugin/`。

### 5.6 教程三：带 Web UI + API 的插件 —— 前端铁律

前端页面放在 `web/` 目录，通过 iframe 嵌入面板主区域。**铁律：插件页面内的 AJAX 必须用相对路径**：

```javascript
// ✅ 正确：相对路径 —— iframe 地址是 /p/my-plugin/，相对路径解析为 /p/my-plugin/api/xxx
const data = await fetch('api/data');

// ❌ 错误：绝对路径会打到面板核心 404（面板的 /api/* 是核心自己的 API，不是你的）
const data = await fetch('/api/data');
```

同理，页面里引用自己的静态资源也用相对路径：

```html
<!-- ✅ -->
<script src="js/app.js"></script>
<link rel="stylesheet" href="css/style.css">
```

Go 插件里用 `//go:embed web` 内嵌前端再按路径读出即可（参考 `plugins/hello/main.go` 的 `handleIndex`）。

### 5.7 菜单、多页面

`menus` 支持多个条目，每个菜单一个路径：

```yaml
menus:
  - title: 概览
    icon: 📊
    path: /
    section: tools
  - title: 配置
    icon: ⚙️
    path: /settings
    section: tools
```

面板侧边栏会为你的插件生成一个分组（插件名 + 运行状态圆点 + ⚙ 管理按钮），组内列出所有菜单项；点击后 iframe 加载 `/p/<插件名>/<path>`。`section` 字段当前为扩展预留（侧边栏暂按插件分组），写 `tools` 即可。

### 5.8 日志与调试

- 插件 stdout/stderr → `<面板家目录>/logs/plugins/<插件名>.log`。
- 面板内查看：侧边栏插件详情（⚙）→ 日志；或 API `GET /api/plugins/{name}/log`；或命令行 `panel log -n 200`。
- **独立调试**（不经过面板，直接跑你的插件）：

```bash
cd my-plugin
PLUGIN_PORT=19090 PANEL_HOME=/tmp/mp-dev ./bin/my-plugin
# 或 shell 插件：
PLUGIN_PORT=19090 bash bin/start.sh
curl http://127.0.0.1:19090/
```

### 5.9 打包与分发（三种方式）

**插件包格式**：`.tar.gz`，**顶层必须是一个目录**（目录名 = 插件名），内含 `manifest.yaml`。例如：

```bash
tar czf my-plugin.tar.gz my-plugin/     # my-plugin/manifest.yaml 必须存在
```

分发方式：

1. **拷贝即安装**：把插件目录放进 `<面板家目录>/plugins/<name>/`，重启面板自动登记。
2. **URL 安装**：把 `.tar.gz` 传到任意 URL（含 GitHub Release），在面板「插件」页粘贴地址 + **可选 SHA256 校验值**。
3. **官方插件内嵌**：加入 `plugins/` 源码树，`build.sh` 编译期自动压缩内嵌（见 6.3）。

> 插件以面板同权限运行（root）。**只安装你信任的插件**；从 URL 安装务必填 SHA256，并检查包内容。

### 5.10 常见坑清单

| 坑 | 说明 |
|---|---|
| 前端用了绝对路径 | iframe 内必须用相对路径（见 5.6） |
| 没监听 `$PLUGIN_PORT` | 面板等 6 秒端口未就绪会判启动失败并杀掉进程 |
| 忽略 SIGTERM | 面板停止/重启时插件进程会被拖 3 秒后 SIGKILL，可能丢数据 |
| `bind: 0.0.0.0` 且未加鉴权 | 插件端口直接暴露给网络——终端类插件建议保持 127.0.0.1 |
| 端口写死 | 用 `$PLUGIN_PORT`，不要写死 19000 |
| manifest 缺 `name`/`command` | 插件不会被识别（登记时跳过） |
| 标题/描述里带 HTML | 面板前端会转义，但请保持纯净文本 |
| WebSocket 被拒 | 终端类 WS 需校验 Origin 与面板同源；参考 `plugins/terminal` |

### 5.11 安全须知

- 面板与插件都以 **root** 运行，进程隔离 ≠ 安全沙箱：**安装任意插件 ≈ 交出 root**。
- 插件默认只监听 `127.0.0.1`，外部流量必须走面板网关（登录后）——不要随意改成 `0.0.0.0`。
- 插件页面输出的用户可控内容（文件名、日志等）渲染到 DOM 时要做转义，避免 XSS（面板前端用 `esc()` 的同一思路）。
- 分发插件包时提供 SHA256，安装方校验后使用。

---

## 6. 核心开发指南（改框架）

### 6.1 新增一个 API 端点（手把手）

以新增 `GET /api/hello` 为例：

1. **写 handler**（`internal/api/` 下，可加到任意文件）：

```go
// internal/api/hello.go
package api

import "net/http"

// handleHello 示例端点。
func (s *Server) handleHello(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, map[string]any{"hello": "world"})
}
```

2. **注册路由**（`internal/api/server.go` 的 `Handler()` 里，加一行）：

```go
	mux.HandleFunc("GET /api/hello", s.auth(s.handleHello)) // 需要登录
	// 不需要登录的（如向导接口）就不包 s.auth
```

3. **前端调用**（`internal/embed/web/js/app.js`）：

```javascript
const data = await api('/api/hello'); // api() 是封装好的 fetch（带 cookie、报错处理）
```

4. 重新构建 + 验证：

```bash
./build.sh && PANEL_HOME=/tmp/mp-dev ./bin/panel
curl -b <cookie> http://127.0.0.1:8787/api/hello
```

### 6.2 新增一个设置项

1. 存取：`db` 的 `settings` 表，`s.db.GetSetting("my_key")` / `s.db.SetSetting("my_key", v)`。
2. 接口：在 `handleSettingsGet` 返回值里加字段；在 `handleSettingsPut` 解析并校验。
3. 前端：`renderSettings()` 加对应控件，PUT 时带上字段。

### 6.3 构建 / 打包 / CI

- **`./build.sh`**：编译 `plugins/` 下所有插件（可用 `PLUGINS="hello"` 指定子集）→ gzip 压缩 → 写入 `internal/embed/plugins/` → 编译核心 → `bin/panel`。
- **`./package.sh`**：按平台打包（linux-amd64/arm64、windows-amd64、darwin-amd64），版本号取自 `config.go` 的 `Version` 或 `--version`；产出 `dist/*.tar.gz` + `.sha256`。注意各平台插件清单不同（Windows 只有 hello，因为纯标准库）。
- **CI**（`.github/workflows/build.yml`）：`./build.sh` → `go vet ./...` → `go test ./...`。推送后自动跑。
- **注意**：`internal/embed/plugins/` 是构建产物（gitignore），`go test`/`go build` 前需要先跑一次 `./build.sh` 生成它。

### 6.4 测试

```bash
go test ./...          # 全量
go test ./internal/auth ./internal/db   # 指定包
```

现有测试：`internal/auth`（口令哈希新旧格式、令牌签发/校验）、`internal/db`（用户/插件/会话 CRUD、.bak 回退、.tmp 清理）、`internal/api`（CSRF 同源判定、插件包解压防御）、`internal/plugins`（端口探测、日志轮转、端口分配、进程身份校验）。给新功能补测试时参考这些文件（同包测试可直接用未导出函数）。

### 6.5 发布流程（版本号 → 打包 → Release）

1. **升版本号**：改 `internal/config/config.go` 的 `Version`，同步更新 README 中的下载链接版本。
2. **自测**：`./build.sh && go vet ./... && go test ./...`。
3. **打包**：`./package.sh`（或 `--targets` 指定平台）。
4. **打 tag + 推送**：

```bash
git add -A && git commit -m "feat/fix: ..."
git push origin main
git tag v0.3.2 && git push origin v0.3.2
```

5. **发 Release**：GitHub → Releases → 新建 v0.x.y，把 `dist/` 下的 4 个 `.tar.gz` + 4 个 `.sha256` 全部作为附件上传（install.sh 会自动校验 SHA256）。
6. 发布完**及时撤销**用过的 GitHub 令牌。

---

## 7. 常见问题 FAQ

**Q：面板起不来，提示端口占用？**
`LISTEN_ADDR` 被占用。换端口：`LISTEN_ADDR=127.0.0.1:8788 ./bin/panel`，或改 `.env`。

**Q：插件点了没反应 / 502？**
看插件日志 `logs/plugins/<name>.log`：最常见是入口路径错（`command` 相对插件目录）、没监听 `$PLUGIN_PORT`、或启动超时。

**Q：忘记管理员密码？**
直接编辑 `data/panel.json` 删掉 `users` 数组（会丢失所有账户），重启面板重新走初始化向导。会话也建议一并清空。

**Q：日志无限变大？**
核心与插件日志超 20MB 会在启动时轮转（保留 `.1`）。长期运行的实例可自行加 cron 轮转。

**Q：怎么升级面板？**
下载新版安装包重复安装即可：只替换 `bin/panel`，`.env` / `panel.json` / 插件目录均保留。

**Q：`go test` 报 `pattern ./...: directory prefix . does not contain main module`？**
在仓库根目录（有 `go.mod`）执行；先跑 `./build.sh` 生成内嵌插件目录。

**Q：插件要对外提供 SMTP/Webhook 端口？**
把 manifest 的 `bind` 改为 `0.0.0.0`，插件自己监听对应端口；注意自行加鉴权，并知悉这会绕过面板网关。

**Q：改了核心代码但插件行为没变？**
官方插件在 `plugins/` 源码树，改完必须重跑 `./build.sh`（会重新编译并内嵌插件包），只 `go build` 核心不会带上新插件。

---

## 附录：官方插件清单（现成参考实现）

| 插件 | 语言 | 亮点（值得抄的部分） |
|---|---|---|
| `hello` | Go | 最小插件骨架；`//go:embed` 前端；优雅退出 |
| `file-manager` | Go | 文件浏览/上传/下载/编辑；`FM_ROOT` 限制根目录；相对路径 AJAX 范例 |
| `resource-monitor` | Go | 读 `/proc` 做监控页；定时刷新 |
| `terminal` | Go | WebSocket + PTY 网页终端；WS Origin 校验；前端 lib/ 静态资源引用 |
