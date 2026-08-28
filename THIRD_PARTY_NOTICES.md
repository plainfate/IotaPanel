# Third-Party Notices

本仓库除自身 Apache-2.0 代码外，内嵌/依赖了以下第三方组件，其许可声明保留如下。

## 前端组件

### xterm.js（plugins/terminal/web/lib/）
Copyright (c) 2017-2025 The xterm.js authors (https://github.com/xtermjs/xterm.js)
Licensed under the MIT License:
- Permission is hereby granted, free of charge, to any person obtaining a copy of
  this software and associated documentation files (the "Software"), to deal in
  the Software without restriction, including without limitation the rights to
  use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
  of the Software, and to permit persons to whom the Software is furnished to do
  so, subject to the following conditions:
- The above copyright notice and this permission notice shall be included in all
  copies or substantial portions of the Software.
- THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.

## Rust 依赖

本项目为纯 Rust 实现，核心 `core` 与全部插件通过 Cargo 引入以下第三方 crate（Apache-2.0 / MIT 双许可以下均遵守原作者条款）：

- **serde / serde_json** — Apache-2.0 或 MIT，Copyright (c) Serde Developers / David Tolnay（版本化 JSON 序列化）
- **sha2 / sha1** — Apache-2.0 或 MIT，Copyright (c) RustCrypto Project（HMAC-SHA2 / SHA-1）
- **hmac / pbkdf2** — Apache-2.0 或 MIT，Copyright (c) RustCrypto Project（会话签名的 HMAC 与密码 PBKDF2）
- **hex** — Apache-2.0 或 MIT，Copyright (c) Koki Kato（十六进制编解码）
- **base64** — Apache-2.0 或 MIT，Copyright (c) Alice Maz / Marshall Pierce（Base64 编解码，multipart / 证书处理）
- **subtle** — Apache-2.0 或 MIT，Copyright (c) RustCrypto Project（恒定时间比较，令牌/签名校验）
- **getrandom** — Apache-2.0 或 MIT，Copyright (c) The getrandom contributors（安全随机源）
- **flate2 / tar** — Apache-2.0 或 MIT，Copyright (c) RustCrypto / Alexis Beingessner（tarball 插件包解压）
- **libc** — Apache-2.0 或 MIT，Copyright (c) The Rust Project Developers（平台系统调用，终端 PTY / 资源统计 / 文件系统）
- **rustls / rustls-pemfile** — Apache-2.0 或 ISC（部分），Copyright (c) rustls developers（https-front 的 TLS 终止）
- **rcgen** — Apache-2.0，Copyright (c) rcgen developers（https-front 自签证书生成）
- **ring** — ISC，Copyright (c) Brian Smith（rustls 的密码学后端；https-front 编译期需要 gcc）
- **tempfile**（仅 Dev 依赖）— Apache-2.0 或 MIT，Copyright (c) Steven Allen / David Hotham（单元测试临时目录）

完整清单与精确版本见 `Cargo.lock`。以上各 crate 的许可全文及版权声明可在其源码仓库获取。

本项目自身采用 Apache-2.0 许可证，全文见 `LICENSE`。
