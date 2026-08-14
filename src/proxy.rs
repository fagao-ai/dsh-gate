use axum::{
    body::Body,
    extract::{FromRequestParts, Request, State, ws::{Message, WebSocket, WebSocketUpgrade}},
    http::{StatusCode, header, Uri},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use std::time::Duration;
use tokio_tungstenite::tungstenite;
use crate::AppState;

#[derive(Clone)]
pub struct ProxyState {
    pub backend_origin: String,      // connect target, e.g. "http://host.docker.internal:3080"
    pub rewrite_host: String,        // Host header sent upstream: always the loopback form
    pub rewrite_origin: String,      // Origin header sent upstream: always the loopback form
    pub client: Client<HttpConnector, Body>,
}

impl ProxyState {
    pub fn new(backend: &str) -> Self {
        let url: http::Uri = backend.parse().expect("invalid BACKEND url");
        let authority = url.authority().expect("BACKEND needs authority").as_str();
        // The connect target may be host.docker.internal / a LAN IP, but the
        // Host dsh sees must look loopback or its /api trust fence and
        // loopback-pinned privileged RPC answer 403. Reuse the same port.
        let port = authority.rsplit(':').next().unwrap_or("3080");
        let rewrite_host = format!("127.0.0.1:{port}");
        let rewrite_origin = format!("http://{rewrite_host}");
        let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
        Self { backend_origin: backend.to_string(), rewrite_host, rewrite_origin, client }
    }
}

fn is_websocket_upgrade(req: &Request<Body>) -> bool {
    let upgrade = req.headers().get(header::UPGRADE).and_then(|v| v.to_str().ok()).unwrap_or("");
    upgrade.to_ascii_lowercase() == "websocket"
}

/// Fallback handler: forward everything (HTTP and WebSocket) to the backend.
pub async fn proxy(State(state): State<AppState>, req: Request<Body>) -> Response {
    if is_websocket_upgrade(&req) {
        let (mut parts, body) = req.into_parts();
        let uri = parts.uri.clone();
        match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
            Ok(ws) => {
                return ws.on_upgrade(move |socket| pump_ws(state.proxy, socket, uri)).into_response();
            }
            Err(_) => {
                // Not a valid upgrade; fall through to the HTTP proxy.
                let rebuilt = Request::from_parts(parts, body);
                return http_proxy(state.proxy, rebuilt).await;
            }
        }
    }
    http_proxy(state.proxy, req).await
}

async fn http_proxy(px: ProxyState, mut req: Request<Body>) -> Response {
    // Rewrite the request to the backend, keeping the path/query. Host and
    // Origin are rewritten so the dsh /api trust fence sees a loopback
    // authority (the gateway owns authentication; the fence has nothing to
    // defend against) — this also unlocks loopback-pinned privileged RPC.
    let target = format!("{}{}", px.backend_origin, req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    match target.parse::<Uri>() {
        Ok(uri) => *req.uri_mut() = uri,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    }
    req.headers_mut().insert(header::HOST, px.rewrite_host.parse().unwrap());
    if req.headers_mut().contains_key(header::ORIGIN) {
        req.headers_mut().insert(header::ORIGIN, px.rewrite_origin.parse().unwrap());
    }
    match tokio::time::timeout(Duration::from_secs(15), px.client.request(req)).await {
        Ok(Ok(res)) => {
            let (mut parts, body) = res.into_parts();
            match tokio::time::timeout(Duration::from_secs(15), body.collect()).await {
                Ok(Ok(collected)) => {
                    let bytes = collected.to_bytes();
                    tracing::debug!("proxy ok: status={} len={}", parts.status, bytes.len());
                    // Transfer-encoding/connection belong to the upstream hop; let
                    // axum frame the response body itself.
                    parts.headers.remove(header::TRANSFER_ENCODING);
                    parts.headers.remove(header::CONNECTION);
                    Response::from_parts(parts, Body::from(bytes))
                }
                Ok(Err(e)) => {
                    tracing::warn!("upstream body error: {e}");
                    (StatusCode::BAD_GATEWAY, format!("upstream body error: {e}")).into_response()
                }
                Err(_) => {
                    tracing::warn!("upstream body timeout");
                    StatusCode::GATEWAY_TIMEOUT.into_response()
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!("upstream unavailable: {e}");
            (StatusCode::BAD_GATEWAY, format!("upstream unavailable: {e}")).into_response()
        }
        Err(_) => {
            tracing::warn!("upstream request timeout");
            StatusCode::GATEWAY_TIMEOUT.into_response()
        }
    }
}

/// Bidirectional WebSocket pump: client <-> backend. Either direction closing
/// (or the backend refusing the upgrade) tears the other side down.
async fn pump_ws(px: ProxyState, client: WebSocket, uri: Uri) {
    let target = format!("ws://{}{}", px.rewrite_host, uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    let backend = match tokio_tungstenite::connect_async(&target).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::warn!("ws backend connect failed: {e}");
            return;
        }
    };
    let (mut client_tx, mut client_rx) = client.split();
    let (mut backend_tx, mut backend_rx) = backend.split();

    let c2b = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            let m = axum_to_tungstenite(msg);
            let closing = matches!(m, tungstenite::Message::Close(_));
            if backend_tx.send(m).await.is_err() || closing {
                break;
            }
        }
    };
    let b2c = async {
        while let Some(Ok(msg)) = backend_rx.next().await {
            let m = tungstenite_to_axum(msg);
            let closing = matches!(m, Message::Close(_));
            if client_tx.send(m).await.is_err() || closing {
                break;
            }
        }
    };
    tokio::select! {
        _ = c2b => {}
        _ = b2c => {}
    }
    // Best-effort graceful close of the peer socket after the other side died.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = client_tx.close().await;
        let _ = backend_tx.close().await;
    }).await;
}

fn axum_to_tungstenite(m: Message) -> tungstenite::Message {
    match m {
        Message::Text(t) => tungstenite::Message::Text(tungstenite::protocol::frame::Utf8Bytes::from(t.to_string())),
        Message::Binary(b) => tungstenite::Message::Binary(b),
        Message::Ping(p) => tungstenite::Message::Ping(p),
        Message::Pong(p) => tungstenite::Message::Pong(p),
        Message::Close(c) => tungstenite::Message::Close(c.map(|f| tungstenite::protocol::CloseFrame {
            code: f.code.into(),
            reason: tungstenite::protocol::frame::Utf8Bytes::from(f.reason.to_string()),
        })),
    }
}

fn tungstenite_to_axum(m: tungstenite::Message) -> Message {
    match m {
        tungstenite::Message::Text(t) => Message::Text(axum::extract::ws::Utf8Bytes::from(t.to_string())),
        tungstenite::Message::Binary(b) => Message::Binary(b),
        tungstenite::Message::Ping(p) => Message::Ping(p),
        tungstenite::Message::Pong(p) => Message::Pong(p),
        tungstenite::Message::Close(c) => Message::Close(c.map(|f| axum::extract::ws::CloseFrame {
            code: f.code.into(),
            reason: axum::extract::ws::Utf8Bytes::from(f.reason.to_string()),
        })),
        tungstenite::Message::Frame(_) => unreachable!("no raw frames in client mode"),
    }
}
