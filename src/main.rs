use std::env;

use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
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
    let path = req.uri().path();
    if path == "/login" || path == "/logout" {
        return next.run(req).await;
    }
    match state.auth.session_id_from_cookie(req.headers()) {
        Some(_) => next.run(req).await,
        None => Redirect::to("/login").into_response(),
    }
}
