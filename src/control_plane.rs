use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    sync::mpsc::Receiver,
    time::{sleep, Duration},
};

use crate::db::DBMessage;

const REFRESH_NEIGHBORS_TIMEOUT: u64 = 10 * 60 * 1000;
const ANTI_ENTROPY_TIMEOUT_MS: u64 = 600 * 1 * 1000;
const PORT: u64 = 3000;
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

    fn is_outdated(&self,  incoming: HashMap<String, u64>) -> bool {
        let clock = self.clock.lock().expect("unable to acquire lock");
        incoming.iter().any(|(key, &incoming_val)| {
            match clock.get(key) {
                Some(&local_val) => incoming_val > local_val,
                None => true
            }
        })
    }

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
                    // we take a snapshot of the neighbors, rather than holding the lock
                    let neighbors = {
                        let n = self.neighbors.lock().expect("failed to acquire lock");
                        n.clone()
                    };

                    for i in 0..neighbors.len() {
                        println!("{}", neighbors[i]);
                            let n = neighbors[i].clone();
                            let endpoint = format!("http://{}:{}/get_clock", n, PORT);
                            let resp = reqwest::get(endpoint)
                                .await
                                .expect("failed to send message")
                                .json::<HashMap<String, u64>>()
                                .await
                                .expect("failed to parse json");

                            // the incoming clock is newer
                            if self.is_outdated(resp) {
                                // we must update our entries first, THEN our keys
                                let endpoint = format!("http://{}:{}/get_recent_clipboard", n, PORT);
                                // TODO: actually parse / transmit data
                                // let data = reqwest::get(endpoint)
                                //     .await
                                //     .expect("failed to send message")
                                //     .json()
                                //     .await
                                //     .expect("failed to parse json");

                                // for pair in data {
                                //     match pair.value {
                                //         image => {}
                                //
                                //     }
                                // }

                            }
                            // self.update_clock(resp)
                    }
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
