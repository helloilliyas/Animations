//! Runs the app in a desktop browser: serves `app/ui` and exposes the same
//! command API as the Android app at `POST /api/<command>`.
//!
//! Usage: cargo run -p cse-dev-server [-- DB_PATH [ADDR]]
//! Defaults: ./cse-trader-dev.sqlite, 127.0.0.1:8787

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cse_service::AppService;
use serde_json::{json, Value};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let db = args
        .next()
        .unwrap_or_else(|| "cse-trader-dev.sqlite".into());
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8787".into());

    let service = Arc::new(AppService::open(&db).expect("open database"));
    tokio::spawn(service.clone().run_poller());

    let ui = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/ui");
    let app = Router::new()
        .route("/api/{cmd}", post(command))
        .fallback_service(ServeDir::new(ui))
        .with_state(service);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("CSE Trader dev server on http://{addr} (database {db})");
    axum::serve(listener, app).await.expect("server");
}

async fn command(
    State(service): State<Arc<AppService>>,
    Path(cmd): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let args = body.map(|Json(v)| v).unwrap_or(Value::Null);
    match service.dispatch(&cmd, args).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
