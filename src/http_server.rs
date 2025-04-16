use std::collections::HashMap;

use axum::{routing::get, Extension, Json, Router};
use tokio::sync::{mpsc::{Receiver, Sender}, oneshot};

use crate::db::{DBMessage, ClipboardEntry};


async fn health_check() -> &'static str {
    "hai"
}

async fn clock() -> Json<HashMap<String, u64>> {
    let data = HashMap::new();
    Json(data)
}

async fn recent_clipboard(Extension(tx): Extension<Sender<DBMessage>>) -> Json<Vec<(ClipboardEntry, String)>> {
    let (x, y) = oneshot::channel();
    let msg = DBMessage {
        cmd: crate::db::DBCommand::Recent {length: 100},
        sender: x
    };
    tx.send(msg).await.expect("failed to send db message");

    let resp = y.await.expect("failed to read response");
    if let Ok(crate::db::Response::Recent { values }) = resp {
        for x in &values {
            println!("{:?}", x);
        }
        Json(values)
    }
    else {
        Json(Vec::new())
    }

}

pub async fn run_http_server(tx: Sender<DBMessage>) {
    let app = Router::new()
        //.nest()
        .route("/health", get(health_check))
        .route("/clock", get(clock))
        .route("/recent_clipboard", get(recent_clipboard))
        .layer(Extension(tx));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    println!("running on localhost:3000");
    axum::serve(listener, app).await.expect("failed to start server");
}
