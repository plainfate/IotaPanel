# IotaPanel（微面板）

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Go](https://img.shields.io/badge/Go-1.27+-00ADD8.svg)](https://go.dev/)
[![Rust](https://img.shields.io/badge/Rust-1.85+-DEA584.svg)](https://rust-lang.org/)
[![GitHub](https://img.shields.io/badge/GitHub-plainfate%2FIotaPanel-181717.svg?logo=github)](https://github.com/plainfate/IotaPanel)

轻量级、插件化的服务器管理面板。微内核设计：**核心只做认证、反代与插件进程管理**，功能全部以插件形式扩展——插件是独立 OS 进程，**任意语言可写**（Go / Rust / Python / Shell / Node.js……），崩溃隔离、按需冷启动。

- 常驻内存：Rust 核心 **~3MB** / Go 核心 ~8MB
- 内置插件：文件管理、资源监控、网页终端、HTTPS 网关、**MCP Agent（AI 客户端可操控面板）**
- 平台：Linux（x86_64 / ARM64）、Windows、macOS

## 特性

- **插件 = 独立同级进程**：任意语言，崩溃隔离。
- **按需冷启动**：开机只运行核心；点菜单才拉起插件（约 1-2 秒）；空闲自动退出释放内存。
- **原生 UI 融合**：安装插件后自动向侧边栏注入菜单，页面经反向代理嵌入主内容区，地址栏不跳转。
- **插件自由**：从 URL / GitHub Release 安装插件包（可选 SHA256 校验），或手动放入插件目录即装即用。
- **插件仓库**由@BCZZB维护 https://iotapanel.plainfate.top/
- **插件列表**(只做统计请自行审查安全性)由@plainfate列出  https://github.com/plainfate/iotapanel-list

- **HTTPS 一键开启**：内置 https-front 插件（自签 / 已有证书 / Let's Encrypt ACME），无需外部反代。
- **内置 MCP Agent**：Cherry Studio 等 AI 客户端经 MCP 协议读取/控制面板（Bearer 令牌认证，工具开关可配）。
- **双核心**：Go 核心（主力）+ Rust 核心（rust-core/，v0.4.0），同一插件契约、同一数据格式，可互切。

> 资源占用、冷启动耗时等数据为 **linux/arm64 · Go 1.27 / Rust 1.98 实测值**，不同平台/Go 版本会略有差异，仅供参考。
> 感谢此项目的贡献者@bczzb@li63050a@vexify-coder@vexify-root

## 快速开始

> 说明：TLS 由内置 https-front 插件提供（自签 / 已有证书 / Let's Encrypt ACME），无需外部反代；也可在 Nginx/Caddy 反代层终结。面板部署在受信反代之后时，设置 `PANEL_TRUST_PROXY=1` 以正确识别 HTTPS 与原始域名。首次启动会进入初始化向导（创建管理员）。

### 方式一：下载安装包

从 [Releases](https://github.com/plainfate/IotaPanel/releases) 下载对应平台的最新包（附 `.sha256` 校验文件）：

| 平台 | 包名 |
|---|---|
| Linux x86_64 | `iotapanel-0.3.12-linux-amd64.tar.gz` |
| Linux ARM64 | `iotapanel-0.3.12-linux-arm64.tar.gz` |
| Windows x64 | `iotapanel-0.3.12-windows-amd64.tar.gz` |
| macOS x64 | `iotapanel-0.3.12-darwin-amd64.tar.gz` |

```bash
tar xzf iotapanel-0.3.12-linux-amd64.tar.gz
cd iotapanel-0.3.12-linux-amd64
./install.sh -d /data/panel          # 安装为 systemd 服务（开机自启，自动移除旧版服务名）
# 或直接开发运行：PANEL_HOME=/data/panel ./bin/panel
```

Windows：解压后直接运行 `bin\panel.exe`（首次访问提示初始化）。

### 方式二：一行命令自动安装（install.sh 自动下载、解压并 SHA256 校验）

**先获取 install.sh**（它随安装包分发，也可直接从仓库获取，二选一）：

```bash
# 方式 A：从仓库直接下载
curl -fLO https://raw.githubusercontent.com/plainfate/IotaPanel/main/install.sh
# 方式 B：克隆仓库（拿到 install.sh 后进入目录）
git clone https://github.com/plainfate/IotaPanel.git && cd IotaPanel
```

**再执行一行安装**（Linux，两种架构任选其一）：

```bash
bash install.sh -d /data/panel --url https://github.com/plainfate/IotaPanel/releases/download/v0.3.12/iotapanel-0.3.12-linux-arm64.tar.gz   # ARM64
bash install.sh -d /data/panel --url https://github.com/plainfate/IotaPanel/releases/download/v0.3.12/iotapanel-0.3.12-linux-amd64.tar.gz   # x86_64
```

Windows / macOS 包内**没有 install.sh**，需手动解压后直接运行（同上方式一）。

### 方式三：本地构建后安装（仅开发者/内测）

环境：Go 1.27+（或 rust-core 用 Rust 1.85+），详见 [DEVELOPMENT.md](DEVELOPMENT.md)。

```bash
./build.sh                      # 编译核心 + 内嵌插件
PANEL_HOME=/tmp/mp-dev LISTEN_ADDR=127.0.0.1:8787 ./bin/panel
```

## 插件

- **内置插件**：hello（示例）、file-manager（文件管理）、resource-monitor（资源监控）、terminal（网页终端）、https-front（HTTPS 网关）、mcp-agent（MCP AI Agent）
- **安装**：插件页支持从 URL / GitHub Release 安装 `.tar.gz` 包（可选 SHA256 校验）；或手动把插件目录放入 `<安装目录>/plugins/`。
- **开发**：任意语言，实现 `manifest.yaml` + HTTP 服务即可，完整规范见 [DEVELOPMENT.md](DEVELOPMENT.md)。

## MCP Agent（AI 客户端操控面板）

面板内置 mcp-agent 插件，让 AI 客户端通过 MCP 协议读取/控制面板。

### 面板侧准备
1. 插件页启动 **MCP Agent**（保活常驻）。
2. 侧边栏「MCP Agent」页复制**访问令牌**。
3. （可选）写操作：编辑 `<安装目录>/etc/mcp-agent/config.yaml` 填 `admin_password`（管理员密码）后重启插件；`allow_shell` 高危默认关。

### Cherry Studio 配置
1. **设置 → MCP 服务器 → 添加**
2. 填写：
   - 名称：`iotapanel`
   - 类型：**HTTP**
   - URL：`http://<服务器IP>:8787/p/mcp-agent/mcp`
   - Headers：`{"Authorization": "Bearer <访问令牌>"}`
3. 保存后，新建对话并选择该 MCP 服务器。
4. 提问示例：「查看服务器状态」「列出已安装插件」「重启 hello 插件」。

> 说明：MCP 写操作使用**API 会话**（v0.3.9+），不会把管理员网页登录会话踢下线；`admin_password` 仅在服务端配置文件保存，配置接口回显掩码。
> ⚠️ 安全：mcp-agent 声明了 `auth: none`（`/mcp` 端点绕过面板登录、仅靠 Bearer 令牌保护）。第三方插件若也声明 `auth: none`，等于把该端点直接开放到公网——**仅当插件自带强鉴权时才应使用**。

### 远程连接
若面板仅监听本机（https-front 一键收紧后），远程客户端需走 HTTPS 入口：`https://<域名或IP>:8443/p/mcp-agent/mcp`；自签证书可能被客户端拒绝，建议用 ACME 正式证书。

## 双核心（Go 主力 + Rust 实验）

| | Go 核心（主力，正式发布） | Rust 核心（rust-core/） |
|---|---|---|
| 状态 | 正式发布（最新 v0.3.12） | v0.4.0（本地，待发布） |
| 功能 | 完整 | 完整核心能力（认证/向导/管理前端/MCP 写操作） |
| 内存 | ~8MB | ~3MB |
| 平台 | linux/win/mac（Go 原生交叉） | linux/win（macOS 需 macOS 构建） |
| 打包 | `./package.sh` → `iotapanel-<版本>-*` | `rust-core/package.sh` → `iotapanel-rust-0.4.0-*` |

**打包分离**：两个核心各自独立出包、独立发布，互不影响；升级/安装互不干扰（同样是按架构选包）。

## Rust 核心（rust-core/）

`rust-core/` 提供了 Rust 重写的微内核核心（v0.4.0），与 Go 版**同一插件契约、同一数据格式**，可直接运行全部官方插件。

- 常驻内存 **~3MB**（Go 版约 8MB），二进制约 3.2MB
- 已实现：认证会话（单会话/API 会话/CSRF）、初始化向导、改密、插件启停/保活/空闲退出、`auth: none`（MCP 免登录直连）、面板 API（mcp-agent 写操作可用）、内嵌 Web 管理前端
- 兼容实测：6 个官方插件全部通过（含 mcp-agent 写操作端到端验证）；同一 `panel.json`/`etc/.env` 可双核心互切不丢登录态
- 尚未实现：插件商店/URL 安装、部分高级面板 API、macOS 构建（需 mac 或 CI）
- 详细文档/构建/打包见 [rust-core/README.md](rust-core/README.md)

## 安全

- 口令：PBKDF2-SHA256（60 万次迭代，旧版本自动升级）；会话：HMAC 签名 + 服务端指纹；CSRF：Origin 校验
- 单账号单会话；API 会话（MCP 等）互不踢
- 防 XSS / 点击劫持 / MIME 嗅探响应头；HTTPS 与受信反代下的 HSTS
- 插件网关默认需登录；`auth: none` 白名单化（仅 MCP 等自带鉴权插件）

## 开源许可

[Apache-2.0](LICENSE)，第三方组件声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)（xterm.js 等）。开发者指南与插件契约详见 [DEVELOPMENT.md](DEVELOPMENT.md)。