use anyhow::Context;
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
pub async fn start_http_server() -> anyhow::Result<()> {
    let app = Router::new().route("/readyz", get(health_check));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .unwrap();

    axum::serve(listener, app).await.context("serve http server")?;
    Ok(())
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
