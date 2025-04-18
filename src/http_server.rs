use std::collections::HashMap;

use axum::{response::IntoResponse, routing::{get, post}, Extension, Json, Router};
use http::StatusCode;
use tokio::sync::{
    mpsc::Sender,
    oneshot,
};

use crate::{
    control_plane::{PeerInfo, ControlMessage, Gossip},
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

async fn gossip(
    Extension(tx): Extension<Sender<ControlMessage>>,
    Json(payload): Json<Gossip>
) -> impl IntoResponse {
    println!("got request");
    let Gossip { clock, entry, ttl } = payload;
    let cur_clock = {
        let (x, y) = oneshot::channel();
        let msg = ControlMessage {
            cmd: crate::control_plane::ControlCommand::GetClock,
            sender: x
        };
        tx.send(msg).await.expect("failed to send msg");
        y
            .await
            .expect("failed to recieve msg")
            .expect("could not get clock")
    };
    if let crate::control_plane::Response::Clock { data } = cur_clock {
        let mut res = StatusCode::OK;
        if crate::control_plane::is_outdated(&data, &clock) && ttl > 0 {
            let (x, y) = oneshot::channel();
            let msg = ControlMessage {
                cmd: crate::control_plane::ControlCommand::Transmit {
                    data: entry, ttl: Some(ttl - 1)
                },
                sender: x
            };
            tx.send(msg).await.expect("failed to send msg");
            let resp = y.await.expect("failed to send msg");
            res = match resp {
                Ok(crate::control_plane::Response::OK) => {
                    StatusCode::OK
                },
                Err(e) => {
                    eprintln!("{}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
                _ => {
                    unreachable!()
                }
            }
        };
        res
    }
    else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

pub async fn run_http_server(dtx: Sender<DBMessage>, ctx: Sender<ControlMessage>) {
    let app = Router::new()
        //.nest()
        .route("/health", get(health_check))
        .route("/clock", get(clock))
        .route("/recent_clipboard", get(recent_clipboard))
        .route("/neighbors", get(neighbors))
        .route("/gossip", post(gossip))
        .layer(Extension(dtx))
        .layer(Extension(ctx));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("running on localhost:3000");
    axum::serve(listener, app)
        .await
        .expect("failed to start server");
}
