# MicroPanel（微面板）

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Go](https://img.shields.io/badge/Go-1.25+-00ADD8.svg)](https://go.dev/)
[![GitHub](https://img.shields.io/badge/GitHub-plainfate%2FMicroPanel-181717.svg?logo=github)](https://github.com/plainfate/MicroPanel)

> **极简微内核 + 进程级隔离 + 按需启动** 的 Linux 服务器应用框架。
> 它不是传统面板，而是服务器领域的「Chrome 浏览器」——核心只负责开窗口（网关），网页（功能）由社区插件无限创造。

> ⚠️ **公网部署必须前置 HTTPS 反向代理**（Nginx / Caddy 等）：面板默认以明文 HTTP 监听 `:8787`，不经加密代理直接暴露公网时，管理员口令与登录会话可被网络嗅探。
> 面板本身暂不内置 TLS/ACME（Roadmap），请在反代层终结 TLS；面板部署在受信反代之后时，设置 `PANEL_TRUST_PROXY=1` 以正确识别 HTTPS 与原始域名。

- **微内核**：常驻内存仅约 8MB（Go 编译的单一二进制，内嵌前端与官方插件包）。
- **插件 = 独立同级进程**：任意语言（Go、Rust、Python、Node.js、Shell…），崩溃隔离。
- **按需冷启动**：开机只运行核心；点菜单才拉起插件（约 1-2 秒）；空闲自动退出释放内存。
- **原生 UI 融合**：安装插件后自动向侧边栏注入菜单，页面经反向代理嵌入主内容区，地址栏不跳转。
- **插件自由**：从 URL / GitHub Release 安装插件包（可选 SHA256 校验），或手动放入插件目录即装即用。

> 资源占用、冷启动耗时等数据为 **linux/arm64 · Go 1.27 实测值**，不同平台/Go 版本会略有差异，仅供参考。

---

## 快速开始

### 1. 本地构建（需要 Go 1.25+）

```bash
./build.sh
# 产物：bin/panel（自包含二进制，内嵌前端 + 官方插件包）
```

### 2. 直接运行开发版

```bash
PANEL_HOME=/tmp/mp-dev ./bin/panel
# 默认在全部网卡监听 :8787（IPv4+IPv6 双栈），浏览器打开 http://<服务器IP>:8787 进入初始化向导
# 仅本机调试: PANEL_HOME=/tmp/mp-dev LISTEN_ADDR=127.0.0.1:8787 ./bin/panel
```

> **监听地址说明**：`LISTEN_ADDR` 支持三种写法——`:8787` 全部网卡（IPv4+IPv6 双栈，默认）、`0.0.0.0:8787` 仅 IPv4、`127.0.0.1:8787` 仅本机。
> 改监听地址（已通过 install.sh 安装时）：编辑安装目录 `etc/.env` 中的 `LISTEN_ADDR`，然后 `systemctl restart micropanel`。

### 3. 正式安装（从 GitHub Release 安装，路径自定义）

> ⚠️ 面板**不自动选择架构**：请按服务器 CPU 架构选择安装包（`x86_64` → amd64，`aarch64/arm64` → arm64）。
> 安装脚本会自动做两件事：**SHA256 校验**（`--url` 安装且发布包附带 `.sha256` 时）与**二进制自检**（`-version`，防架构不匹配/文件损坏）。

#### 方式一：手动安装（推荐，共 4 步）

**第 1 步：下载对应架构的安装包**
下面以 Linux ARM64 为例。其他平台把文件名换成：
- Linux x86_64 → `micropanel-0.3.2-linux-amd64.tar.gz`
- Windows → `micropanel-0.3.2-windows-amd64.tar.gz`
- macOS → `micropanel-0.3.2-darwin-amd64.tar.gz`

```bash
curl -fLO https://github.com/plainfate/MicroPanel/releases/download/v0.3.2/micropanel-0.3.2-linux-arm64.tar.gz
```

**第 2 步：解压**

```bash
tar xzf micropanel-0.3.2-linux-arm64.tar.gz
```

**第 3 步：进入解压目录**

```bash
cd micropanel-0.3.2-linux-arm64
```

**第 4 步：运行安装脚本**（默认装到 /data/panel 并注册 systemd 自动启动）

```bash
bash install.sh                          # 默认安装
bash install.sh --port 8787              # 自定义端口
bash install.sh -d /srv/panel            # 自定义安装目录
bash install.sh --no-systemd             # 只部署，不注册 systemd
```

#### 方式二：一行命令自动安装（install.sh 自动下载、解压并 SHA256 校验）

Linux（两种架构任选其一）：

```bash
bash install.sh -d /data/panel --url https://github.com/plainfate/MicroPanel/releases/download/v0.3.2/micropanel-0.3.2-linux-arm64.tar.gz   # ARM64
bash install.sh -d /data/panel --url https://github.com/plainfate/MicroPanel/releases/download/v0.3.2/micropanel-0.3.2-linux-amd64.tar.gz   # x86_64
```

Windows / macOS 包内**没有 install.sh**，需手动解压后直接运行：

```bash
# Windows x64
curl -fLO https://github.com/plainfate/MicroPanel/releases/download/v0.3.2/micropanel-0.3.2-windows-amd64.tar.gz
tar xzf micropanel-0.3.2-windows-amd64.tar.gz   # 或右键「全部解压」
cd micropanel-0.3.2-windows-amd64 && bin\panel.exe

# macOS x64
curl -fLO https://github.com/plainfate/MicroPanel/releases/download/v0.3.2/micropanel-0.3.2-darwin-amd64.tar.gz
tar xzf micropanel-0.3.2-darwin-amd64.tar.gz
cd micropanel-0.3.2-darwin-amd64 && bin/panel
```

#### 方式三：本地构建后安装（仅开发者/内测）

```bash
./build.sh && bash install.sh -d /data/panel
```

**升级** = 下载新版本安装包，重复方式一（或方式二指定新包地址）：只替换 `bin/panel`，`.env` / `panel.json` / 插件目录均不受影响。


### 4. 打包与发布（多平台）

```bash
./package.sh                              # 打包全部平台: linux-amd64 linux-arm64 windows-amd64 darwin-amd64
./package.sh --targets linux-amd64,linux-arm64   # 只打指定平台
./package.sh --version 0.3.2              # 自定义版本号
```

产物（`dist/`，附 `.sha256`）：

| 包 | 平台 | 内容 |
|---|---|---|
| `micropanel-<版本>-linux-amd64.tar.gz` | x86_64 Linux | 全部插件（含终端）+ install.sh |
| `micropanel-<版本>-linux-arm64.tar.gz` | ARM64 Linux | 全部插件（含终端）+ install.sh |
| `micropanel-<版本>-windows-amd64.tar.gz` | x64 Windows | 核心 + Hello（纯标准库） |
| `micropanel-<版本>-darwin-amd64.tar.gz` | x64 macOS | 核心 + 基础插件 |

> 安装方法见上文「3. 正式安装」；Windows / macOS 包不含 install.sh，解压后直接运行 `bin/panel(.exe)`，首次访问走 Web 初始化向导。

### 5. 日常使用

1. 浏览器访问面板 → 首次进入初始化向导（管理员账号 + 勾选基础插件）。
2. 侧边栏点击插件菜单 → 首次约 1-2 秒冷启动 → 页面无缝嵌入主区域。
3. 插件详情（点侧边栏插件名旁的 ⚙）：保活开关、启动/停止/重启、日志、卸载。
4. 「插件」页可从 URL / GitHub Release 安装插件包（粘贴 URL，可选填 SHA256 自动校验），手动放入 plugins/ 目录亦会自动登记。
5. 设置页可调整「空闲退出时间」（默认 5 分钟）并查看 port-map.json 端口映射表。

---

## 物理目录结构（用户完全掌控）

```text
/data/panel/                     # 用户自定义安装位置
├── bin/
│   └── panel                    # 核心二进制（约 18MB，唯一常驻进程，空闲约 8MB）
├── etc/
│   ├── .env                     # PANEL_HOME、LISTEN_ADDR、JWT_SECRET、IDLE_TIMEOUT
│   └── port-map.json            # 端口映射表（插件名 -> {端口, PID}）
├── plugins/                     # 每个插件一个独立文件夹
│   ├── file-manager/
│   │   ├── manifest.yaml        # 元信息（菜单、版本、入口）
│   │   ├── bin/file-manager     # 插件进程（任意语言）
│   │   └── web/                 # 插件自带前端资源
│   └── hello/
├── data/
│   └── panel.json               # 轻量 JSON 存储：用户、插件、会话、设置
└── logs/
    ├── panel.log                # 核心日志
    └── plugins/<name>.log       # 各插件进程输出
```

---

## 核心架构

### 微内核职责（仅此三项）

| 模块 | 职责 |
|---|---|
| 用户认证 | PBKDF2 口令哈希 + HMAC 签名会话 cookie |
| 反向代理网关 | `/p/<插件名>/*` → 插件进程端口（`httputil.ReverseProxy`） |
| 插件进程管理 | 冷启动、空闲退出、保活、端口映射 |

不含任何具体运维功能（文件管理、数据库、防火墙…）——那都是插件的活。

### 进程管理机制

1. **默认休眠**：开机只运行核心，所有插件进程都不启动。
2. **冷启动（端口认领）**：请求 `/p/<name>/` 时核心同步拉起插件进程：
   - 在端口池（默认 19000-19999）选一个空闲端口；
   - 注入环境变量 `PLUGIN_PORT`、`PLUGIN_BIND`、`PLUGIN_NAME`、`PANEL_HOME`、`MICROPANEL_VERSION`；
   - 执行 manifest 里的 `command`，轮询等待端口就绪（约 1-2 秒）；
   - 写入 `port-map.json`。
3. **空闲退出**：插件无请求超过 `IDLE_TIMEOUT`（默认 5 分钟）即被回收。实现为**事件驱动**：每次请求重置计时器，无常驻巡检协程、无心跳轮询，开销压到最低。
4. **保活开关**：开启后进程常驻；核心收到 SIGTERM 时跳过保活插件，**核心重启后由 `port-map.json` 认领端口继续使用，不杀进程**。
5. **启动复用**：核心启动时扫描 `port-map.json`，端口仍被占用（上次崩溃残留）直接复用；失效记录清理。

### 插件 = 同级进程

- 插件是核心拉起的**独立进程**，跑在独立端口；核心只通过环境变量 + 端口映射与其通信，不共享内存、不做进程内调用。
- 插件崩溃/被杀 → 核心毫发无伤，下次点击自动重新拉起；核心重启也不会中断已开启保活的插件（进程保留、端口复用）。
- 任何语言：只要监听 `$PLUGIN_PORT` 就是一个合法插件（`manifest.command` 指向任意可执行文件或脚本即可；官方 `hello` 插件为 Go 实现，早期版本是纯 Shell + Python3）。

---

## 插件开发（SDK）

每个插件 = 一个目录 + `manifest.yaml`：

```yaml
name: my-plugin          # 唯一标识（目录名）
title: 我的插件           # 侧边栏显示名
version: 0.1.0
author: 某开发者
description: 一句话描述
language: go             # 语言标记（仅展示用）
command: bin/my-plugin   # 入口（可执行文件或脚本，相对插件目录）
bind: 127.0.0.1          # 插件监听地址（默认本机；外部流量统一走面板网关，更安全）
menus:                   # 注入侧边栏的菜单（可多个）
  - title: 我的插件
    icon: 🧩
    path: /              # 插件页面内的路径（iframe 指向 /p/<插件名>/<path>）
    section: tools
```

核心注入的环境变量：

| 变量 | 说明 |
|---|---|
| `PLUGIN_PORT` | 分配的端口，**必须监听它** |
| `PLUGIN_BIND` | 监听地址（= manifest.bind） |
| `PLUGIN_NAME` | 插件名 |
| `PANEL_HOME` | 面板安装目录 |
| `MICROPANEL_VERSION` | 核心版本 |

> **安全默认**：插件默认只监听 `127.0.0.1`——外部流量只能走面板网关（`/p/<插件名>/*`），插件不直接暴露。
> 若插件需要对外直连（如邮件服务的 SMTP 端口、Webhook 回调），在 manifest 中把 `bind` 改为 `0.0.0.0` 或具体网卡 IP 即可。
>
> ⚠️ **安全提醒**：面板以 root 运行，插件拥有与面板相同的权限（进程隔离 ≠ 安全沙箱）——**只安装你信任的插件**；从 URL 安装时请填写 SHA256 校验值，并核对插件包内容。

### 第三方插件分发（拷贝即安装）

第三方插件打包 = 一个目录 + `manifest.yaml`（入口可以是任意语言编译的二进制或脚本）：

```text
my-plugin/
├── manifest.yaml     # 元信息（name 需与目录名一致）
├── bin/my-plugin     # 可执行入口（任意语言）
└── web/              # 插件自带前端（可选）
```

分发方式：

1. **拷贝即安装**：把插件目录放进 `/data/panel/plugins/<name>/`，重启面板后核心自动扫描登记，侧边栏即刻出现菜单。示例：`scp -r my-plugin root@server:/data/panel/plugins/`。
2. **离线目录**：`tar czf my-plugin.tar.gz my-plugin/` 发给用户解压到插件目录即可。
3. **URL / GitHub**：插件作者把 .tar.gz 发布到任意 URL（含 GitHub Release 直链），使用者在面板「插件」页粘贴地址（可选填 SHA256）即可安装。

> 提示：面板核心只负责「分配端口 + 拉起进程 + 反向代理」，不关心插件用什么语言写。
> 开发时可在插件目录直接运行 `PLUGIN_PORT=19000 PANEL_HOME=/data/panel ./bin/my-plugin` 独立调试。

---

## 面板控制命令（CLI）

安装后 `panel` 命令已软链到 `/usr/local/bin/panel`，任意目录可直接使用；未安装时用构建产物 `./bin/panel` 代替。

```bash
panel status      # 查看状态：安装目录/监听地址/进程 PID/运行中插件数
panel start       # 启动（systemd 安装走 systemctl，否则分离式后台启动）
panel stop        # 停止（保活插件进程保留，重启后端口复用）
panel restart     # 重启
panel log         # 查看核心日志（panel log -n 200 指定行数）
panel uninstall   # 卸载面板（停止服务、移除 systemd 与命令，数据保留）
panel version     # 版本号
panel help        # 帮助
```

systemd 安装时 `start`/`stop`/`restart` 等价于 `systemctl {start,stop,restart} micropanel`；`status`/`log` 仍直接读取面板自身，无需 systemctl。
登录安全：**单账号单会话**（新登录自动踢掉旧会话）；登录页可勾选「**记住我**」（30 天免登录），不勾选则为会话级（关浏览器即需重新登录）。

---

## REST API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/login` / `/api/logout` | 登录 / 退出 |
| GET | `/api/status` | 核心状态（版本、运行插件数…） |
| GET | `/api/setup/state` | 是否已初始化 |
| POST | `/api/setup/start` | 初始化向导（建管理员 + 批量装插件） |
| GET | `/api/setup/status` | 安装进度轮询 |
| GET | `/api/plugins` | 已安装插件列表（含菜单、运行状态） |
| POST | `/api/plugins/{name}/start` `/stop` `/restart` | 进程控制 |
| POST | `/api/plugins/{name}/keepalive` | 保活开关 |
| GET | `/api/plugins/{name}/log` | 插件日志 |
| DELETE | `/api/plugins/{name}` | 卸载 |
| POST | `/api/store/install-url` | 从 URL 安装插件包（含 SHA256 校验） |
| POST | `/api/store/{name}/install` | 安装/更新内嵌官方插件 |
| GET/PUT | `/api/settings` | 空闲退出时间等设置 |
| GET | `/api/account` | 账户信息（用户名/创建/最近登录） |
| POST | `/api/account/password` | 修改密码（改密后其他会话强制下线） |
| GET | `/api/account/sessions` | 登录会话列表 |
| POST | `/api/account/sessions/revoke` | 强制下线指定会话 |
| POST | `/api/account/sessions/revoke-all` | 下线除当前外全部会话 |
| GET/PUT | `/api/security` | 登录安全策略（失败次数上限/锁定分钟） |
| GET | `/p/{name}/*` | 插件页面反向代理入口 |

---

## 与主流面板的本质区别

| 维度 | 主流面板 | MicroPanel |
|---|---|---|
| 核心职责 | 集成所有功能 | 仅网关 + 进程调度 |
| 安装位置 | 强制固定目录 | 安装时任意选择 |
| 资源策略 | 所有服务随面板常驻 | 按需冷启动，闲置自动释放 |
| 插件语言 | 通常限 Python/PHP | 任意语言 |
| 崩溃隔离 | 插件可能拖垮面板 | 独立进程，毫发无伤 |
| 扩展方式 | 官方维护有限插件 | 插件自由（URL/GitHub/手动目录） |
| 升级成本 | 覆盖安装，配置易丢 | 仅替换 bin/panel |

---

## 项目结构

```text
cmd/panel/                 # 核心入口
internal/
  config/                  # .env 与运行配置
  db/                      # 轻量 JSON 存储（用户/插件/会话/设置）
  auth/                    # PBKDF2 + 会话 cookie
  plugins/                 # 插件管理器（生命周期 + 安装/卸载）
  gateway/                 # 反向代理网关
  api/                     # REST API 与页面路由
  embed/                   # 内嵌前端 + 官方插件包（build.sh 生成）
plugins/                   # 官方插件源码
  file-manager/            # Go：文件管理
  resource-monitor/        # Go：资源监控
  hello/                   # Go：极简保活示例（约 7MB）
  terminal/                # Go：网页终端（Linux，xterm.js + PTY）
```
（面板前端位于 internal/embed/web/，纯 HTML/CSS/JS，编译期内嵌进核心二进制。）

## 路线图（Roadmap）

- [x] M1 微内核：认证、网关、进程管理、初始化向导
- [x] M2 插件系统：manifest、冷启动、空闲退出、保活、URL 安装
- [ ] M3 官方插件生态：数据库、防火墙、定时任务、邮件…
- [ ] M4 插件分发：远程仓库索引、插件签名（当前支持 URL/GitHub 直装 + SHA256）
- [ ] M5 安全加固：CSRF 防护、插件权限声明、审计日志


---

## 许可证（License）

本项目采用 **GNU 通用公共许可证第 3 版（GPLv3）**（SPDX: `GPL-3.0-or-later`）。
您可以自由使用、修改、复制与分发本软件，但修改版本或衍生作品必须同样以 GPLv3 授权并开放源代码。
本软件按「现状」提供，不附带任何担保。完整法律文本见 [LICENSE](LICENSE)。

版权所有（Copyright ©）2026 [plainfate](https://github.com/plainfate)，遵循 GPLv3 许可证发布。
