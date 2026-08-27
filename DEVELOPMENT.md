# 开发指南

## 环境

需要 Rust stable、Cargo 和 gcc。`rustls/ring` 在编译 https-front 时需要 C 编译器；Linux 下可用 `gcc`，若系统只有 gcc 而没有 `cc`，设置 `CC=gcc`。

```bash
export CARGO_INCREMENTAL=0
export CC=gcc
export AR=/usr/bin/ar
```

## 构建

```bash
./build.sh                 # release，输出到 bin/
./build.sh --debug         # debug
cargo test --workspace
cargo build --workspace
```

`core/src/embedded_data.rs` 使用 `include_bytes!` 编译期内嵌面板前端；官方插件的页面由各插件自己的 `include_str!` 内嵌。修改 `web/` 或插件 `web/` 后重新构建即可。

## 本地运行

```bash
PANEL_HOME=/tmp/iota-panel LISTEN_ADDR=:8787 bin/iotapanel serve
```

首次访问 `/` 会进入 `/setup`。初始化向导通过 `POST /api/setup/start` 创建管理员并安装选择的官方插件。

## 插件协议

每个插件是独立 Cargo crate，依赖 `iotapanel-sdk`，并在 `manifest.yaml` 声明名称、版本、命令和保活策略。核心为插件进程注入：

- `PANEL_HOME`
- `PLUGIN_HOME`
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
