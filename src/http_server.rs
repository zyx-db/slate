use std::collections::HashMap;

use axum::{routing::get, Extension, Json, Router};
use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
};

use crate::{
    control_plane::{PeerInfo, ControlMessage},
    db::{ClipboardEntry, DBMessage, Clock},
};

async fn health_check() -> &'static str {
    "hai"
}

async fn clock(Extension(tx): Extension<Sender<ControlMessage>>) -> Json<Clock> {
    let (x, y) = oneshot::channel();
    tx.send(ControlMessage {
        cmd: crate::control_plane::ControlCommand::GetClock,
        sender: x,
    })
    .await
    .expect("failed to send control message");

    let response = y.await.expect("failed to get response");
    if let Ok(crate::control_plane::Response::Clock { data }) = response {
        Json(data)
    } else {
        eprintln!("failed to get clock?");
        let data = HashMap::new();
        Json(data)
    }
}

async fn recent_clipboard(
    Extension(tx): Extension<Sender<DBMessage>>,
) -> Json<Vec<(ClipboardEntry, String)>> {
    let (x, y) = oneshot::channel();
    let msg = DBMessage {
        cmd: crate::db::DBCommand::Recent { length: 100 },
        sender: x,
    };
    tx.send(msg).await.expect("failed to send db message");

    let resp = y.await.expect("failed to read response");
    if let Ok(crate::db::Response::Recent { values }) = resp {
        Json(values)
    } else {
        Json(Vec::new())
    }
}

async fn neighbors(Extension(tx): Extension<Sender<ControlMessage>>) -> Json<Vec<PeerInfo>> {
    let (x, y) = oneshot::channel();
    let msg = ControlMessage {
        cmd: crate::control_plane::ControlCommand::GetNeighbors,
        sender: x,
    };
    tx.send(msg).await.expect("failed to send db message");

    let resp = y.await.expect("failed to read response");
    if let Ok(crate::control_plane::Response::Neighbors { info }) = resp {
        Json(info)
    } else {
        Json(Vec::new())
    }
}

pub async fn run_http_server(dtx: Sender<DBMessage>, ctx: Sender<ControlMessage>) {
    // TODO:
    // handle POST /gossip
    // body contains a Clock, ClipboardEntry, and a TTL
    // if the clock is old, ignore
    // update values as needed
    // if TTL > 0, retransmit
    let app = Router::new()
        //.nest()
        .route("/health", get(health_check))
        .route("/clock", get(clock))
        .route("/recent_clipboard", get(recent_clipboard))
        .route("/neighbors", get(neighbors))
        .layer(Extension(dtx))
        .layer(Extension(ctx));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("running on localhost:3000");
    axum::serve(listener, app)
        .await
        .expect("failed to start server");
}
