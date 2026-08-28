// SPDX-License-Identifier: Apache-2.0
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! IotaPanel SDK：核心与官方插件共用的迷你 HTTP/WebSocket 基础设施。
//!
//! - [`http`]:阻塞式 HTTP/1.1 服务器（线程池模型，Keep-Alive、分块请求解码）
//! - [`ws`]:WebSocket 服务器端实现（握手 + RFC6455 帧编解码），支持代理透传
//! - [`util`]:Base64 / 十六进制 / MIME / 环境变量等工具函数

pub mod http;
pub mod util;
pub mod ws;
