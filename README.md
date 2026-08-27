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
sudo apt-get install -y gcc  # 或 build-essential

./build.sh          # release 构建，产物在 bin/
```

### 安装

```bash
sudo ./install.sh                     # 安装 + systemd 启动
sudo ./install.sh --home /data/panel  # 指定数据目录
```

安装完成后打开 `http://<IP>:8787`，进入初始化向导（创建管理员账号 + 勾选预装插件）。

### 手动运行

```bash
# 前台
PANEL_HOME=~/.iotapanel bin/iotapanel serve

# 或以守护方式
PANEL_HOME=~/.iotapanel nohup bin/iotapanel serve >/tmp/iotapanel.log 2>&1 &
```

### CLI

```bash
bin/iotapanel version   # 版本
bin/iotapanel status    # 面板与插件状态
bin/iotapanel log -n 50 # 查看核心日志
bin/iotapanel stop      # 停止
bin/iotapanel restart   # 重启
bin/iotapanel uninstall # 卸载（保留数据）
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
   (华丽的独立进程，经 stdin/env 注入配置)
```

- **`sdk/`**：共享基础库。迷你 HTTP 服务器（线程每连接）、WebSocket 帧编解码、YAML 解析、工具函数。核心与插件共用。
- **`core/`**：面板核心。配置、数据库（JSON 持久化）、认证（PBKDF2 + HMAC 会话）、插件管理器、反向代理网关、安装器、REST API、前端 Web UI。
- **`plugins/`**：6 个官方插件，各自独立编译、以独立进程运行。

### 插件怎么写（任意 Rust）

每个插件是个独立 crate，`Cargo.toml` 依赖 `iotapanel-sdk`，`manifest.yaml` 描述元信息，监听 `PLUGIN_PORT` 提供服务即可。面板会注入 `PLUGIN_PORT / PLUGIN_BIND / PLUGIN_NAME / PLUGIN_HOME / PANEL_HOME` 环境变量，并经 `/p/<名称>/` 把请求转发进来。

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

## 📦 插件商店

面板自带 6 个官方内嵌插件，可在初始化向导或「插件商店」页勾选/安装；也支持通过 URL 安装 tarball 插件包。

## 🔐 安全说明

- 密码使用 **PBKDF2-SHA256**（60 万迭代）加盐哈希，旧参数哈希自动升级。
- 登录失败达到阈值自动锁定账号一段时间（可配置）。
- 会话可管理：查看登录会话、强制下线其它设备、下线所有会话。
- 所有响应附 `X-Frame-Options` / `X-Content-Type-Options` 安全头；非本地受信代理下走 HSTS。

## 📄 License

[Apache-2.0](LICENSE) · 感谢 [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES.md)