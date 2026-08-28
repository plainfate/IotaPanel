# IotaPanel REST API 契约

> 与 Go 版 API 契约逐条对齐（`core/src/server.rs`）。所有请求/响应均为 JSON；除标注外均需登录（`mp_session` Cookie）。

## 认证

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/setup/start` | 初始化向导：`{username, password, plugins:[]}`，异步执行，返回 202 |
| GET | `/api/setup/status` | 初始化进度：`{running, done, total, current, complete, error}` |
| GET | `/api/setup/state` | `{configured}` |
| POST | `/api/login` | `{username, password, remember?, api?}`；单账号单会话，踢掉其它非 API 会话 |
| POST | `/api/logout` | 吊销当前会话 |
| GET | `/api/me` | `{username, uid}` |

## 账户 / 会话

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/account` | 账户信息 + 安全策略 |
| POST | `/api/account/username` | `{new_username}`（3-32 字符） |
| POST | `/api/account/password` | `{old_password, new_password}`（≥6 位），改密踢其它会话 |
| GET | `/api/account/sessions` | `{sessions:[{jti,ip,user_agent,created_at,expires_at,current,revoked}]}` |
| POST | `/api/account/sessions/revoke` | `{jti}` 强制下线指定会话 |
| POST | `/api/account/sessions/revoke-all` | 下线其它所有会话 |

## 安全策略

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/security` | `{fail_limit, lock_minutes}` |
| PUT | `/api/security` | 更新登录失败锁定策略 |

## 插件

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/plugins` | 列表：`{plugins:[{name,title,version,author,description,keepalive,menus,status}]}` |
| POST | `/api/plugins/:name/start` | 启动（冷启动，返回 `{port, pid}`） |
| POST | `/api/plugins/:name/stop` | 停止 |
| POST | `/api/plugins/:name/restart` | 重启 |
| POST | `/api/plugins/:name/keepalive` | `{enabled}` 切换保活 |
| GET | `/api/plugins/:name/log` | 插件日志 |
| DELETE | `/api/plugins/:name` | 卸载（停进程 + 删目录 + 删记录） |

## 商店

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/store` | 内嵌插件目录（未初始化时开放给向导） |
| POST | `/api/store/:name/install` | 安装内嵌插件 |
| POST | `/api/store/install-url` | `{url, sha256?}` 远程安装旧生态插件包（tar.gz 单顶层目录 + manifest.yaml，与 Go 版包格式一致） |

## 设置 / 系统

| 方法 | 路径 | 说明 |
|---|---|---|
| GET/PUT | `/api/settings` | 空闲退出分钟、主题（sage/ocean/rose/lilac）、语言、监听端口 |
| GET | `/api/log` | 核心日志末尾 150 行 |
| POST | `/api/system/restart` | 触发面板重启 |

## 网关（插件前端）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET* | `/p/:plugin/...` | 反向代理到插件端口；`manifest.auth=none` 且路径 `/mcp` 时免面板登录 |
| GET* | `/p/:plugin/ws` | WebSocket 字节级桥接（终端等插件依赖） |

注入头：`X-Forwarded-Proto / X-Forwarded-Host / X-Panel-Plugin`；`PANEL_TRUST_PROXY=1` 时信任 `X-Forwarded-*`。

## 插件环境变量（与 Go 版一致）

`PLUGIN_PORT`、`PLUGIN_BIND`、`PLUGIN_NAME`、`PANEL_HOME`、`IOTAPANEL_VERSION`

## 安全

- 密码：PBKDF2-SHA256（60 万次迭代，`iterations:hex` 盐；旧版纯 hex 盐按 10 万次兼容并自动升级）
- 会话：`base64url(json{uid,u,exp,j}) + "." + base64url(HMAC-SHA256)`，与 Go 版格式兼容
- CSRF：写操作校验 Origin == Host；响应附 `X-Frame-Options` / `X-Content-Type-Options`
