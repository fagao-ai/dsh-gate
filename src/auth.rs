use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use rand_core::OsRng;
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;

use crate::state::{random_hex, CsrfStore, RateLimiter, SessionStore};
use crate::AppState;

const SESSION_COOKIE: &str = "dsh_rs_session";
const SESSION_TTL_SECS: u64 = 12 * 3600;
const CSRF_TTL_SECS: u64 = 600;
const LOGIN_PATH: &str = "/login";

#[derive(Clone)]
pub struct AuthState {
    pub username: String,
    pub password_hash: String,
    pub sessions: SessionStore,
    pub csrf: CsrfStore,
    pub limiter: RateLimiter,
}

impl AuthState {
    pub fn new(username: String, password: String) -> Self {
        // Hash once at startup; never persisted.
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .expect("argon2 hashing")
            .to_string();
        Self {
            username,
            password_hash: hash,
            sessions: SessionStore::new(),
            csrf: CsrfStore::new(),
            limiter: RateLimiter::new(5, 300),
        }
    }

    pub fn session_id_from_cookie(&self, headers: &HeaderMap) -> Option<String> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix(&format!("{SESSION_COOKIE}=")) {
                if self.sessions.valid(v) {
                    return Some(v.to_string());
                }
            }
        }
        None
    }
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
    csrf: String,
}

pub async fn login_page(State(state): State<AppState>) -> Response {
    let auth = state.auth;
    let token = auth.csrf.issue(std::time::Duration::from_secs(CSRF_TTL_SECS));
    login_html(&token, None, "").into_response()
}

pub async fn login_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let auth = state.auth;
    let ip = client_ip(&headers);
    if auth.limiter.locked(&ip) {
        return login_html("", Some("尝试次数过多，请稍后再试"), &form.username).into_response();
    }
    if !auth.csrf.consume(&form.csrf) {
        return login_html("", Some("表单已过期，请重试"), &form.username).into_response();
    }
    let parsed = argon2::password_hash::PasswordHash::new(&auth.password_hash).ok();
    let ok = form.username == auth.username
        && parsed.map(|p| Argon2::default().verify_password(form.password.as_bytes(), &p).is_ok())
            .unwrap_or(false);
    if !ok {
        if let Some(secs) = auth.limiter.record_failure(&ip) {
            tracing::warn!("login locked for {ip} for {secs}s");
        }
        return login_html("", Some("用户名或密码错误"), &form.username).into_response();
    }
    auth.limiter.reset(&ip);
    let sid = random_hex(24);
    auth.sessions.insert(sid.clone(), form.username.clone(), std::time::Duration::from_secs(SESSION_TTL_SECS));
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_static("/")),
            (
                header::SET_COOKIE,
                HeaderValue::from_str(&format!("{SESSION_COOKIE}={sid}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_TTL_SECS}")).unwrap(),
            ),
        ],
    ).into_response()
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = state.auth;
    if let Some(sid) = auth.session_id_from_cookie(&headers) {
        auth.sessions.remove(&sid);
    }
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_static(LOGIN_PATH)),
            (
                header::SET_COOKIE,
                HeaderValue::from_str(&format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")).unwrap(),
            ),
        ],
    ).into_response()
}

fn client_ip(headers: &HeaderMap) -> String {
    // Trust Cloudflare's header when present (edge terminates TLS and passes the client).
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string())
}

fn login_html(csrf: &str, error: Option<&str>, username: &str) -> axum::response::Html<String> {
    let error_html = error.map(|e| format!(r#"<p class="error">{e}</p>"#)).unwrap_or_default();
    axum::response::Html(format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>DeepSeek Harness · 访问验证</title>
<style>
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; min-height: 100vh; display: flex; align-items: center; justify-content: center;
    font: 14px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif;
    background: #f5f6f8; color: #1f2329; }}
  .card {{ width: min(360px, 92vw); padding: 32px 28px; background: #fff; border-radius: 12px;
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.08); }}
  h1 {{ margin: 0 0 4px; font-size: 20px; }}
  .sub {{ margin: 0 0 20px; color: #646a73; }}
  label {{ display: block; margin-bottom: 6px; font-weight: 600; }}
  input {{ width: 100%; padding: 9px 12px; margin-bottom: 14px; border: 1px solid #d0d3d9;
    border-radius: 8px; font-size: 14px; }}
  button {{ width: 100%; padding: 10px; border: 0; border-radius: 8px; background: #2563eb;
    color: #fff; font-size: 14px; font-weight: 600; cursor: pointer; }}
  button:hover {{ background: #1d4ed8; }}
  .error {{ margin: 0 0 14px; padding: 8px 10px; border-radius: 8px; background: #fdecea; color: #c0392b; }}
</style></head>
<body><main class="card">
  <h1>DeepSeek Harness</h1>
  <p class="sub">此界面受密码保护，请输入用户名和密码。</p>
  {error}
  <form method="post" action="{LOGIN_PATH}">
    <input type="hidden" name="csrf" value="{csrf}">
    <label for="username">用户名</label>
    <input id="username" name="username" value="{username}" autocomplete="username" required autofocus>
    <label for="password">密码</label>
    <input id="password" name="password" type="password" autocomplete="current-password" required>
    <button type="submit">进入</button>
  </form>
</main></body></html>"#,
        csrf = csrf,
        error = error_html,
        username = username,
    ))
}
