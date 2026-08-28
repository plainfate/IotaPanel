# IotaPanel

> 极简、纯 Rust 的多功能设备控制面板（Web 管理界面）。

IotaPanel 是一个**微内核架构**的轻量面板：核心 `core` 只做**认证、反向代理网关、插件进程管理**三件事，所有功能（文件管理、终端、资源监控、HTTPS、MCP Agent…）都由**独立的插件进程**提供。整个项目**零第三方运行时依赖，纯 Rust 编译**。

本项目由原版仓库（Go 实现）**完全重写为 Rust 版**：核心 `core` 与全部 6 个官方插件都由 Go 迁移为 Rust，前端重新设计为现代控制台风格（Monet 多主题 + 中英双语）。

---

## ✨ 特性

- **纯 Rust 实现**：核心 + 全部插件均由 Rust 编写，单二进制 + 小型插件二进制，无 Node/Go/Python 运行时。
- **微内核架构**：面板只负责认证与反向代理；功能全部插件化，可热插拔、可保活、可低占用。
- **反向代理网关**：`/p/<插件名>/...` 统一转发，WebSocket 支持（字节级桥接，供终端等使用）。
- **安全**：PBKDF2-SHA256 密码哈希、登录失败锁定、同源 CSRF 校验、安全响应头、会话管理/强制下线。
- **全新前端**：现代控制台 UI，hash 路由单页，4 套 Monet 主题（松绿/海蓝/玫粉/丁香），简体中文/English 双语，深色模式适配。
- **开箱即用插件**：
  | 插件 | 功能 |
  |---|---|
  | `hello` | 极简保活 & 环境变量演示 |
  | `file-manager` | 浏览/上传/下载/删除/重命名服务器文件 |
  | `resource-monitor` | 实时 CPU/内存/负载/磁盘监控 |
  | `terminal` | 网页终端（xterm.js + WebSocket + PTY） |
  | `https-front` | 为面板提供 HTTPS 入口（自签/已有证书） |
  | `mcp-agent` | MCP 服务器，AI 客户端可读取/控制面板（Bearer 认证） |
- **轻量常驻**：每个插件进程常驻内存约几 MB，冷启动即用。

---

## 🚀 快速开始

### 编译

```bash
# 需要 Rust（1.75+）与 gcc（ring 编译需要）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt-get install -y gcc musl-tools   # musl-tools 提供 musl-gcc（静态链接用）

./build.sh                 # release 构建（本机 native），产物在 bin/
./build.sh --musl          # ★ musl 静态交叉编译（Alpine 部署推荐，零动态依赖）
./build.sh --target aarch64-unknown-linux-musl   # 交叉编译其它架构
./build.sh --debug         # debug 构建
./build.sh --package       # 额外生成 tar.gz 发布包
```

> **为什么用 musl**：旧版默认按 glibc（`x86_64-unknown-linux-gnu`）编译，跑在 Alpine（musl）上会因缺少 `__res_init` 等符号导致**插件进程无法启动**。musl 静态链接后核心与插件均为纯静态 ELF，任意发行版原生运行。

### 安装

从当前仓库源码安装：

```bash
sudo ./install.sh                     # 安装 + systemd 启动（Alpine 自动改用 OpenRC）
sudo ./install.sh --home /data/panel  # 指定数据目录
```

从 GitHub Release 一键安装（自动识别系统与架构：x86_64、aarch64、armv7 或 i686，下载对应 musl 静态包，任意 Linux 发行版通用）：

```bash
curl -sSL https://raw.githubusercontent.com/plainfate/IotaPanel/rust-musl-rewrite/deploy.sh | sudo bash
# 指定版本、数据目录或安装前缀：
sudo bash deploy.sh --version v0.4.0 --home /data/panel --prefix /usr/local
```

Alpine / OpenRC 手动托管（等价于 install.sh 自动生成的配置）：

```bash
apk add --no-cache bash
cat > /etc/init.d/iotapanel <<'EOF'
#!/sbin/openrc-run
name="IotaPanel"
command="/usr/local/bin/iotapanel"
command_args="serve"
command_background="yes"
pidfile="/run/iotapanel.pid"
output_log="/var/log/iotapanel.log"
error_log="/var/log/iotapanel.log"
depend() { need net; }
start_pre() { export PANEL_HOME=/data/panel; }
EOF
chmod +x /etc/init.d/iotapanel
rc-update add iotapanel default && rc-service iotapanel start
```

安装完成后打开 `http://<IP>:8787`，进入初始化向导（创建管理员账号 + 勾选预装插件）。

### 旧生态兼容（Go 版插件直接可用）

本核心按 **Go 版的插件协议与数据格式逐项对齐**，旧 Go 生态的插件无需改动即可迁移：

- **旧插件包**：`tar.gz`（单顶层目录 + `manifest.yaml`）与 Go 版包格式一致，在「插件商店 → URL 安装」处直接安装（`/api/store/install-url`）。
- **拷贝即安装**：把旧插件目录（含 `manifest.yaml` 与 `bin/` 可执行文件）放进 `$PANEL_HOME/plugins/`，核心启动时自动登记。
- **任意语言**：`manifest.yaml` 的 `command` 指向任何可执行文件/脚本（Go/Rust/C/Shell 均可）。
- **数据与账号**：旧 `data/panel.json` 直接读取；旧密码哈希（10 万次迭代）自动升级；会话令牌格式兼容。
- **注入变量**：`PLUGIN_PORT / PLUGIN_BIND / PLUGIN_NAME / PANEL_HOME` 与 Go 版一致。

### 手动运行

```bash
# 前台
PANEL_HOME=~/.iotapanel bin/iotapanel serve

# 或以守护方式
PANEL_HOME=~/.iotapanel nohup bin/iotapanel serve >/tmp/iotapanel.log 2>&1 &
```

### CLI

```bash
bin/iotapanel serve       # 前台运行面板服务
bin/iotapanel status      # 面板状态（核心 + 插件进程）
bin/iotapanel log -n 50   # 查看核心日志（tail N 行）
bin/iotapanel start       # systemd 安装时：启动面板
bin/iotapanel stop        # 停止面板（保留插件进程）
bin/iotapanel restart     # 重启面板
bin/iotapanel uninstall   # 卸载（保留数据）
bin/iotapanel version     # 版本（等价于 -v / --version）
bin/iotapanel help        # 帮助（等价于 -h / --help）
```

---

## 🏗 架构

```
浏览器 ──► core (iotapanel, :8787)
             ├── /               面板 Web UI（内嵌）
             ├── /api/*          REST API（认证/插件/设置/会话/日志…）
             ├── /p/hello/…      反向代理 → 插件 hello
             ├── /p/terminal/…   反向代理 + WebSocket 桥接 → 插件 terminal
             └── 进程管理         拉起/停止/保活/空闲回收各插件
                        │
        ┌───────────────┼────────────────┬───────────────┐
        ▼               ▼                ▼               ▼
   plugin-hello   plugin-file-manager plugin-terminal  plugin-https-front …
   (独立进程，经 env 注入配置；核心自动拉起/保活/空闲回收)
```

- **`sdk/`**：共享基础库。迷你 HTTP 服务器（线程每连接）、WebSocket 帧编解码、YAML 解析、工具函数。核心与插件共用。
- **`core/`**：面板核心。配置、数据库（JSON 持久化）、认证（PBKDF2 + HMAC 会话）、插件管理器、反向代理网关、安装器、REST API、前端 Web UI。
- **`plugins/`**：6 个官方插件，各自独立编译、以独立进程运行。

### 插件怎么写（任意 Rust）

每个插件是个独立 crate，`Cargo.toml` 依赖 `iotapanel-sdk`，`manifest.yaml` 描述元信息，监听 `PLUGIN_PORT` 提供服务即可。面板会注入 `PLUGIN_PORT / PLUGIN_BIND / PLUGIN_NAME / PANEL_HOME` 环境变量，并经 `/p/<名称>/` 把请求转发进来。

```rust
use iotapanel_sdk::http::{Request, Response};
fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or("127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(19000);
    let handler = |r: &Request| Response::html("Hello");
    iotapanel_sdk::http::serve(&bind, port, handler).unwrap();
}
```

更多见 [`DEVELOPMENT.md`](DEVELOPMENT.md)。

---

## ⚙️ 配置

面板通过数据目录下的 `etc/.env` 与环境变量配置（不配置则用默认值）：

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `PANEL_HOME` | 参见 install.sh | 数据目录（数据库 / 插件 / 日志 / 配置） |
| `LISTEN_ADDR` | `:8787` | 面板 HTTP 监听地址 |
| `JWT_SECRET` | 首启动自动生成 | 会话 HMAC 密钥（写入 `etc/.env`） |
| `IDLE_TIMEOUT` | `300`（秒） | 插件空闲回收超时 |
| `PORT_START` / `PORT_END` | `19000` / `19999` | 插件动态端口池 |
| `PANEL_TRUST_PROXY` | 未设置（false） | 设为 `1`/`true` 后信任反向代理，解析 `X-Forwarded-Proto/Host` |

---

## 📦 插件商店

面板自带 6 个官方内嵌插件，可在初始化向导或「插件商店」页勾选/安装；也支持通过 URL 安装 tarball 插件包。

## 🔐 安全说明

- 密码使用 **PBKDF2-SHA256**（60 万迭代）加盐哈希，旧参数哈希自动升级。
- 登录失败达到阈值自动锁定账号一段时间（可配置）。
- 会话可管理：查看登录会话、强制下线其它设备、下线所有会话。
- 所有响应附 `X-Frame-Options` / `X-Content-Type-Options` 安全头；设为可信反向代理（`PANEL_TRUST_PROXY`）时基于 `X-Forwarded-Proto/Host` 恢复真实协议与主机，并预留 HSTS。

## 📄 License

[Apache-2.0](LICENSE) · 本仓库即 MicroPanel 的更名版。前端与终端组件许可见 [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES.md)。