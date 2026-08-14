use std::env;

use axum::{
    Router,
    extract::{Request, State},
    http::{header, HeaderValue, Method, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

mod auth;
mod proxy;
mod state;

#[derive(Clone)]
pub struct AppState {
    pub auth: auth::AuthState,
    pub proxy: proxy::ProxyState,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "dsh_gate=info".into()))
        .init();

    let user = env::var("AUTH_USER").unwrap_or_else(|_| fatal("AUTH_USER"));
    let pass = env::var("AUTH_PASSWORD").unwrap_or_else(|_| fatal("AUTH_PASSWORD"));
    let listen = env::var("LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let backend = env::var("BACKEND").unwrap_or_else(|_| "http://127.0.0.1:3080".to_string());

    let state = AppState {
        auth: auth::AuthState::new(user, pass),
        proxy: proxy::ProxyState::new(&backend),
    };

    let app = Router::new()
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/logout", get(auth::logout))
        .fallback(proxy::proxy)
        .layer(middleware::from_fn_with_state(state.clone(), gate))
        .with_state(state);

    let listener = TcpListener::bind(&listen).await.unwrap_or_else(|e| {
        eprintln!("[dsh-gate] cannot bind {listen}: {e}");
        std::process::exit(1);
    });
    tracing::info!("listening on {listen} -> {backend} (login: /login)");
    axum::serve(listener, app).await.expect("server error");
}

fn fatal(name: &str) -> ! {
    eprintln!("[dsh-gate] missing required environment variable {name}");
    std::process::exit(1);
}

/// Gate every path except the auth endpoints behind a valid session cookie.
async fn gate(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if req.method() == Method::GET && is_pair_landing_path(req.uri()) {
        return pair_landing_page();
    }
    if is_auth_path(req.uri()) || is_mobile_pairing_path(req.uri()) {
        return next.run(req).await;
    }
    match state.auth.session_id_from_cookie(req.headers()) {
        Some(_) => next.run(req).await,
        None => Redirect::to("/login").into_response(),
    }
}

fn is_auth_path(uri: &axum::http::Uri) -> bool {
    matches!(uri.path(), "/login" | "/logout")
}

fn is_pair_landing_path(uri: &Uri) -> bool {
    uri.path() == "/"
        && uri.query().is_some_and(|query| {
            query
                .split('&')
                .any(|part| part == "pair" || part.starts_with("pair="))
        })
}

fn is_mobile_pairing_path(uri: &Uri) -> bool {
    match uri.path() {
        // Pair control endpoints stay exact; only the mobile-owned namespace is prefix-allowed.
        "/api/pair/accept" | "/api/pair/heartbeat" | "/api/pair/status" => true,
        path => is_safe_mobile_namespace_path(path),
    }
}

fn is_safe_mobile_namespace_path(path: &str) -> bool {
    if path == "/m" {
        return true;
    }
    if !path.starts_with("/m/") || path.contains('%') || path.contains('\\') {
        return false;
    }
    path.split('/').all(|segment| segment != "." && segment != "..")
}

fn pair_landing_page() -> Response {
    let mut response = Html(PAIR_LANDING_HTML).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

const PAIR_LANDING_HTML: &str = r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <meta name="theme-color" content="#f5f6f8">
  <meta name="referrer" content="no-referrer">
  <title>移动端远程控制</title>
  <style>
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 24px;
      background: #f5f6f8; color: #1f2329;
      font: 15px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif; }
    main { width: min(360px, 100%); text-align: center; }
    .spinner { width: 42px; height: 42px; margin: 0 auto 22px; border: 3px solid #dfe3e8;
      border-top-color: #2563eb; border-radius: 50%; animation: spin .8s linear infinite; }
    h1 { margin: 0 0 8px; font-size: 20px; font-weight: 650; }
    p { margin: 0; color: #646a73; }
    main.failed .spinner { animation: none; border: 0; border-radius: 50%; background: #c83c3c;
      color: white; display: grid; place-items: center; font-size: 24px; }
    main.failed .spinner::after { content: "!"; }
    @keyframes spin { to { transform: rotate(360deg); } }
    @media (prefers-reduced-motion: reduce) { .spinner { animation-duration: 1.8s; } }
  </style>
</head>
<body>
  <main id="state" role="status" aria-live="polite">
    <div class="spinner" aria-hidden="true"></div>
    <h1 id="title">正在连接</h1>
    <p id="detail">正在建立移动端会话...</p>
  </main>
  <script>
    (async () => {
      const params = new URLSearchParams(location.search)
      const token = params.get('pair')
      params.delete('pair')
      const cleanQuery = params.toString()
      history.replaceState(null, '', '/' + (cleanQuery ? '?' + cleanQuery : ''))

      const paired = async () => {
        try {
          const response = await fetch('/api/pair/status', { credentials: 'same-origin' })
          const body = await response.json()
          return response.ok && body.paired === true
        } catch {
          return false
        }
      }

      try {
        if (!token) throw new Error('missing token')
        const response = await fetch('/api/pair/accept', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ token }),
        })
        if (!response.ok && !(await paired())) throw new Error('pair rejected')
        location.replace('/m')
      } catch {
        const state = document.getElementById('state')
        state.classList.add('failed')
        document.getElementById('title').textContent = '配对链接已失效'
        document.getElementById('detail').textContent = '请在桌面端刷新二维码后重新扫描。'
      }
    })()
  </script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::{is_auth_path, is_mobile_pairing_path, is_pair_landing_path, PAIR_LANDING_HTML};
    use axum::http::Uri;

    fn uri(value: &str) -> Uri {
        value.parse().expect("valid test URI")
    }

    #[test]
    fn auth_paths_are_limited_to_login_and_logout() {
        assert!(is_auth_path(&uri("/login")));
        assert!(is_auth_path(&uri("/logout")));
        assert!(!is_auth_path(&uri("/")));
        assert!(!is_auth_path(&uri("/login/reset")));
    }

    #[test]
    fn pair_landing_requires_the_exact_query_parameter() {
        assert!(is_pair_landing_path(&uri("/?pair=token")));
        assert!(is_pair_landing_path(&uri("/?workspace=one&pair=token")));
        assert!(!is_pair_landing_path(&uri("/")));
        assert!(!is_pair_landing_path(&uri("/?repair=token")));
        assert!(!is_pair_landing_path(&uri("/m?pair=token")));
    }

    #[test]
    fn mobile_pairing_paths_cover_the_phone_flow() {
        for value in [
            "/m",
            "/m/",
            "/m/mobile.js",
            "/m/mobile.js.map",
            "/m/assets/app.css",
            "/api/pair/accept",
            "/api/pair/heartbeat",
            "/api/pair/status",
            "/m/api/session.list",
        ] {
            assert!(is_mobile_pairing_path(&uri(value)), "{value}");
        }
    }

    #[test]
    fn pair_landing_accepts_without_loading_desktop_assets() {
        assert!(PAIR_LANDING_HTML.contains("fetch('/api/pair/accept'"));
        assert!(PAIR_LANDING_HTML.contains("location.replace('/m')"));
        assert!(!PAIR_LANDING_HTML.contains("/assets/"));
        assert!(!PAIR_LANDING_HTML.contains("/plugins/"));
    }

    #[test]
    fn privileged_and_similarly_named_paths_stay_protected() {
        for value in [
            "/",
            "/?repair=token",
            "/api/pair/issue",
            "/api/pair/stop",
            "/api/pair/events",
            "/m-admin",
            "/mobile",
            "/mapi/session.list",
            "/m/../api/pair/issue",
            "/m/%2e%2e/api/pair/issue",
            "/m/%2E%2E/api/pair/issue",
            "/m/.%2e/api/pair/issue",
            "/m/%2fapi/pair/issue",
        ] {
            assert!(!is_mobile_pairing_path(&uri(value)), "{value}");
        }
    }
}
