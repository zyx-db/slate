use axum::{routing::get, Router};

async fn health_check() -> &'static str {
    "hai"
}

pub async fn run_http_server() {
    let app = Router::new()
        //.nest()
        .route("/health", get(health_check));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
