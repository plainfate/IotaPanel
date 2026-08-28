# 开发指南

## 环境

需要 Rust stable、Cargo 和 gcc。`rustls/ring` 在编译 https-front 时需要 C 编译器；Linux 下可用 `gcc`，若系统只有 gcc 而没有 `cc`，设置 `CC=gcc`。要产出 **musl 静态包**（Alpine 部署）还需 `musl-tools`（提供 `musl-gcc`）：

```bash
sudo apt-get install -y musl-tools        # Ubuntu/Debian
apk add --no-cache musl-dev               # Alpine（本机即 musl，无需交叉）
rustup target add x86_64-unknown-linux-musl
```

## 构建

```bash
./build.sh                     # native release，输出到 bin/
./build.sh --musl              # x86_64 musl 静态交叉编译（部署推荐）
./build.sh --target aarch64-unknown-linux-musl   # 交叉编译其它架构
./build.sh --debug             # debug
./build.sh --musl --package    # 构建 + 打 tar.gz 发布包
./build-release.sh             # 发布包（musl 多架构，见脚本头注释）
cargo test --workspace         # SDK 单元测试
bash tests/smoke.sh            # 端到端冒烟测试（需先构建出 bin/）
```

### 构建流程说明

`build.sh` 四步：① 编 SDK + 插件（不编核心）→ ② 插件二进制 `gzip` 为 `plugins/<name>/bin/<name>.gz`，并运行 `scripts/gen-embedded.py` 生成 `core/src/embedded_data.rs` → ③ 编核心（此时内嵌前端 + 全部插件包）→ ④ 组装 `bin/`。

因此面板「初始化向导 / 插件商店」安装官方插件时，核心会从**内嵌的 gz 包**解压出插件可执行文件——这保证了单二进制分发即可自举安装插件。**改动 `web/`、插件 `web/` 或插件源码后，必须重跑 `./build.sh`**（会重新生成嵌入表）。

## 测试

- `cargo test --workspace`：SDK 单测（URL 编解码 / YAML / WS 握手 / multipart）。
- `bash tests/smoke.sh`：端到端（核心启动 → 初始化 → 登录 → 插件网关 → **旧生态插件包远程安装兼容性**）。

## 本地运行

```bash
PANEL_HOME=/tmp/iota-panel LISTEN_ADDR=:8787 bin/iotapanel serve
```

首次访问 `/` 会进入 `/setup`。初始化向导通过 `POST /api/setup/start` 创建管理员并安装选择的官方插件。

## 插件协议

每个插件是独立 Cargo crate，依赖 `iotapanel-sdk`，并在 `manifest.yaml` 声明名称、版本、命令和保活策略。核心为插件进程注入：

- `PANEL_HOME`
- `PLUGIN_NAME`
- `PLUGIN_BIND`
- `PLUGIN_PORT`

插件应监听 `PLUGIN_BIND:PLUGIN_PORT`，提供 HTTP/JSON API。核心会把 `/p/<plugin>/...` 反向代理到插件；WebSocket 插件由核心做透明字节桥接。

## SDK

`sdk` 提供阻塞式 HTTP/1.1 服务端、请求/响应类型、multipart 解析、WebSocket 握手和帧编解码、简易 YAML、进程与文件工具。插件尽量只使用标准库和 SDK，避免引入大型运行时。

## 目录约定

```text
sdk/                 共享 Rust SDK
core/                面板核心、认证、数据库、网关、安装器
plugins/<name>/      独立官方插件
web/                 核心面板前端
bin/                 build.sh 生成的可执行文件
```

## 验证清单

1. `cargo test --workspace`。
2. `./build.sh` 后确认 `bin/` 包含核心和 6 个插件。
3. 启动临时 `PANEL_HOME`，检查 `/api/setup/state`、`/setup`、CSS/JS 静态资源。
4. 完成初始化并登录，检查 `/api/me`、`/api/plugins` 和 `/p/<plugin>/`。
5. 修改认证、插件管理或网关后，补充对应 Rust 单元测试或 HTTP 冒烟测试。
