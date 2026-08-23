//! Reverse-proxy gateway: `/p/<name>/*` -> `http://<bind>:<port>`.
//! Mirrors `internal/gateway/proxy.go`, including WebSocket upgrade bridging
//! (needed by the terminal plugin) and SSE/long-poll streaming passthrough.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, Response, StatusCode};
use axum::response::Response as AxResponse;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use std::sync::Arc;

use crate::plugins::Manager;

const HOP_HEADERS: [&str; 8] = [
    "connection",
    "upgrade",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
];

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let conn_upgrade = headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|c| c.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    conn_upgrade && upgrade.contains("websocket")
}

/// Capture any request (GET/POST/WS/SSE). Cold-starts the plugin if needed,
/// then proxies to the plugin's bound address. Mirrors Go `gateway.ServeHTTP`.
pub async fn serve(
    mgr: &Arc<Manager>,
    trust_proxy: bool,
    req: Request<Body>,
    name: &str,
    plugin_path: String,
) -> Result<AxResponse, StatusCode> {
    // Cold-start (blocking I/O) off the async core.
    let name_owned = name.to_string();
    let mgr2 = mgr.clone();
    let _rt = tokio::task::spawn_blocking(move || mgr2.start(&name_owned))
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
        .map_err(|e| {
            crate::log_warn(&format!("gateway start failed: {}", e));
            StatusCode::BAD_GATEWAY
        })?;
    mgr.touch(name);

    let status = {
        let mgr2 = mgr.clone();
        let n = name.to_string();
        tokio::task::spawn_blocking(move || mgr2.status(&n))
            .await
            .unwrap_or_default()
    };
    if !status.running {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let bind = {
        let mgr2 = mgr.clone();
        let n = name.to_string();
        tokio::task::spawn_blocking(move || mgr2.bind_of(&n))
            .await
            .unwrap_or_else(|_| "127.0.0.1".to_string())
    };

    let origin = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let proto = proto_of(req.headers(), trust_proxy);

    if is_websocket_upgrade(req.headers()) {
        let query = req.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
        let ws_url = format!("ws://{}:{}{}{}", bind, status.port, plugin_path, query);
        return ws_proxy(req, ws_url).await;
    }

    http_proxy(&bind, status.port, trust_proxy, name, &origin, &proto, plugin_path, req).await
}

fn proto_of(headers: &HeaderMap, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(v) = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) {
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    "http".to_string()
}

/// Plain HTTP/SSE/long-poll streaming proxy via reqwest.
async fn http_proxy(
    bind: &str,
    port: i32,
    trust_proxy: bool,
    name: &str,
    orig_host: &str,
    proto: &str,
    plugin_path: String,
    req: Request<Body>,
) -> Result<AxResponse, StatusCode> {
    let query = req.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
    let url = format!("http://{}:{}{}{}", bind, port, plugin_path, query);

    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let method = req.method().clone();
    let mut builder = client.request(method, &url);
    for (k, v) in req.headers().iter() {
        let hname = k.as_str().to_ascii_lowercase();
        if HOP_HEADERS.contains(&hname.as_str()) || hname == "host" {
            continue;
        }
        builder = builder.header(k.as_str(), v.clone());
    }
    builder = builder.header("X-Forwarded-Proto", proto);
    if !orig_host.is_empty() {
        builder = builder.header("X-Forwarded-Host", orig_host);
    }
    builder = builder.header("X-Panel-Plugin", name);

    let body_stream = req.into_body().into_data_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
    builder = builder.body(reqwest::Body::wrap_stream(body_stream));

    let resp = builder.send().await.map_err(|e| {
        crate::log_warn(&format!("gw upstream error: {}", e));
        StatusCode::BAD_GATEWAY
    })?;

    let status = resp.status();
    let mut rb = Response::builder().status(status);
    for (k, v) in resp.headers().iter() {
        let hname = k.as_str().to_ascii_lowercase();
        if HOP_HEADERS.contains(&hname.as_str()) {
            continue;
        }
        rb = rb.header(k.as_str(), v.clone());
    }
    let _ = trust_proxy;
    let body = Body::from_stream(resp.bytes_stream());
    Ok(rb.body(body).unwrap())
}

// ---------- WebSocket bridging ----------

/// Compute the RFC 6455 `Sec-WebSocket-Accept` from a client key.
fn websocket_accept(key: &str) -> Option<String> {
    use sha1::{Digest, Sha1};
    if key.is_empty() {
        return None;
    }
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let sum = hasher.finalize();
    Some(base64::engine::general_purpose::STANDARD.encode(sum))
}

/// Handle a WebSocket upgrade: send a 101 to the client, then bridge the
/// client socket to the upstream plugin WebSocket in the background.
async fn ws_proxy(mut req: Request<Body>, ws_url: String) -> Result<AxResponse, StatusCode> {
    let key = req
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let accept = websocket_accept(&key).ok_or(StatusCode::BAD_REQUEST)?;

    // Register the client upgrade.
    let on_upgrade = hyper::upgrade::on(&mut req);

    tokio::spawn(async move {
        ws_bridge(on_upgrade, &ws_url).await;
    });

    let mut rb = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "upgrade")
        .header(header::SEC_WEBSOCKET_ACCEPT, accept);
    // Echo the requested subprotocol if the client asked for one.
    if let Some(sp) = req
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
    {
        let first = sp.split(',').next().map(str::trim).unwrap_or("").to_string();
        if !first.is_empty() {
            rb = rb.header("Sec-WebSocket-Protocol", first);
        }
    }
    Ok(rb.body(Body::empty()).unwrap())
}

async fn ws_bridge(on_upgrade: hyper::upgrade::OnUpgrade, ws_url: &str) {
    let mut upstream = match tokio_tungstenite::connect_async(ws_url).await {
        Ok((u, _)) => u,
        Err(e) => {
            crate::log_warn(&format!("gw ws upstream connect failed: {}", e));
            return;
        }
    };
    let upgraded = match on_upgrade.await {
        Ok(u) => u,
        Err(e) => {
            crate::log_warn(&format!("gw ws client upgrade failed: {}", e));
            return;
        }
    };
    let client_stream = tokio_tungstenite::WebSocketStream::from_raw_socket(
        TokioIo::new(upgraded),
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;

    let (mut client_tx, mut client_rx) = client_stream.split();
    let (mut up_tx, mut up_rx) = upstream.split();

    let c2u = tokio::spawn(async move {
        while let Some(Ok(msg)) = client_rx.next().await {
            if up_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    });
    let u2c = tokio::spawn(async move {
        while let Some(Ok(msg)) = up_rx.next().await {
            if client_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    });
    let _ = tokio::try_join!(c2u, u2c);
}