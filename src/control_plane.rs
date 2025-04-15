use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    sync::mpsc::Receiver,
    task,
    time::{sleep, Duration},
};

use crate::db::DBMessage;

const REFRESH_NEIGHBORS_TIMEOUT: u64 = 10 * 60 * 1000;
const ANTI_ENTROPY_TIMEOUT_MS: u64 = 600 * 1 * 1000;
//const ANTI_ENTROPY_TIMEOUT_MS: u64 = 1 * 60 * 1000;

pub struct Node {
    neighbors: Arc<Mutex<Vec<String>>>,
    clock: Arc<Mutex<HashMap<String, u64>>>,
}

impl Node {
    pub fn new() -> Self {
        Node {
            neighbors: Arc::new(Mutex::new(Vec::new())),
            clock: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_clock(&self) {}

    async fn read(&self) {}

    async fn gossip(&self) {}

    async fn reload_neighbors(&self) {}

    pub async fn listen(
        &self,
        mut rx: Receiver<ControlMessage>,
        mut tx: mpsc::Sender<DBMessage<'_>>,
    ) {
        println!("control plane started!");

        //let tx2 = tx.clone();
        //task::spawn(async move {
        //    self.background_events(tx2).await;
        //});

        while let Some(msg) = rx.recv().await {
            println!("recieved command: {:?}", msg.cmd);
            match msg.cmd {
                ControlCommand::AntiEntropy => {
                    self.reload_neighbors().await;
                    msg.sender.send(Ok(Response::OK)).expect("failed to reply");
                }
                _ => {
                    msg.sender.send(Ok(Response::OK)).expect("failed to reply");
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ControlCommand {
    AntiEntropy,
    Transmit {},
}

#[derive(Debug)]
pub enum Response {
    OK,
}

#[derive(Debug)]
pub struct ControlMessage {
    pub cmd: ControlCommand,
    pub sender: oneshot::Sender<Result<Response, String>>,
}

pub async fn trigger_anti_entropy(tx: mpsc::Sender<ControlMessage>) {
    println!("anti entropy trigger started!");
    let duration = Duration::from_millis(ANTI_ENTROPY_TIMEOUT_MS);
    loop {
        sleep(duration).await;
        println!("anti entropy trigger!");
        let (x, y) = oneshot::channel();
        let msg = ControlMessage {
            cmd: ControlCommand::AntiEntropy,
            sender: x,
        };
        tx.send(msg).await.expect("failed to send message");
        let response = y.await.expect("failed to read response");
        match response {
            Ok(s) => {
                println!("{:?}", s);
            }
            Err(e) => {
                println!("{:?}", e);
            }
        }
    }
}
