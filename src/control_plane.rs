use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    sync::mpsc::Receiver,
    time::{sleep, Duration},
    net::UnixStream,
};
use ulid::Ulid;
use serde::{Serialize, Deserialize};

use http::{Request, header::HOST};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};

use crate::db::{DBMessage, ClipboardEntry};

const REFRESH_NEIGHBORS_TIMEOUT: u64 = 10 * 60 * 1000;
const ANTI_ENTROPY_TIMEOUT_MS: u64 = 600 * 1 * 1000;
const PORT: u64 = 3000;
// const ANTI_ENTROPY_TIMEOUT_MS: u64 = 1 * 60 * 1000;

pub struct Node {
    host_name: String,
    neighbors: Arc<Mutex<Vec<PeerInfo>>>,
    clock: Arc<Mutex<HashMap<String, u64>>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PeerInfo {
    HostName: String,
    TailscaleIPs: Vec<String>,
    Online: bool,
}

impl Node {
    pub async fn new() -> Self {
        let host_name = {
            let socket_path = "/var/run/tailscale/tailscaled.sock";
            let url_path = "/localapi/v0/status";
            let uri = Uri::new(socket_path, url_path);

            let req = Request::get(uri)
                .header(HOST, "local-tailscaled.sock")
                .body(Full::new(Bytes::new()))
                .unwrap();

            let client: Client<UnixConnector, Full<Bytes>> = Client::unix();

            // let res = client.get(uri).await.unwrap();
            let res = client.request(req).await.unwrap();
            println!("tailscale local api response status {}", res.status());
            let body = res.collect().await.unwrap().to_bytes();
            println!("tailscale local api body {}", String::from_utf8_lossy(&body));
            let json_value: serde_json::Value = serde_json::from_slice(&body).expect("failed to parse json");

            // Extract just the "Peer" object
            let name_json = &json_value["Self"]["HostName"];
            serde_json::from_value(name_json.clone()).unwrap()
        };
        Node {
            host_name,
            neighbors: Arc::new(Mutex::new(Vec::new())),
            clock: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_clock(&self) {}

    async fn read(&self) {}

    async fn gossip(&self) {}

    async fn reload_neighbors(&self) {
        println!("reloading neighbors");
        let socket_path = "/var/run/tailscale/tailscaled.sock";
        let url_path = "/localapi/v0/status";
        let uri = Uri::new(socket_path, url_path);

        let req = Request::get(uri)
            .header(HOST, "local-tailscaled.sock")
            .body(Full::new(Bytes::new()))
            .unwrap();

        let client: Client<UnixConnector, Full<Bytes>> = Client::unix();

        // let res = client.get(uri).await.unwrap();
        let res = client.request(req).await.unwrap();
        println!("tailscale local api response status {}", res.status());
        let body = res.collect().await.unwrap().to_bytes();
        println!("tailscale local api body {}", String::from_utf8_lossy(&body));
        let json_value: serde_json::Value = serde_json::from_slice(&body).expect("failed to parse json");

        // Extract just the "Peer" object
        let peers_json = &json_value["Peer"];
        let peers: HashMap<String, PeerInfo> = serde_json::from_value(peers_json.clone()).unwrap();

        for (node_key, info) in &peers {
            println!("{}: {:?}", node_key, info);
        }

        let neighbors: Vec<PeerInfo> = peers.into_values().collect();
        let mut cur = self.neighbors.lock().expect("failed to acquire lock");
        cur.clear();
        cur.extend(neighbors);
    }

    fn is_outdated(&self, incoming: &HashMap<String, u64>) -> bool {
        let clock = self.clock.lock().expect("unable to acquire lock");
        incoming.iter().any(|(key, &incoming_val)| {
            match clock.get(key) {
                Some(&local_val) => incoming_val > local_val,
                None => true
            }
        })
    }

    async fn update_values(&self, incoming_updates: &Vec<(ClipboardEntry, String)>, incoming_clock: &HashMap<String, u64>, tx: &mut mpsc::Sender<DBMessage>) {
        for update in incoming_updates {
            let (entry, timestamp) = update;
            let timestamp = Ulid::from_string(&timestamp).expect("failed to parse ulid");
            let (x, y) = oneshot::channel();
            let msg = match entry{
                ClipboardEntry::Image( i ) => {
                    let i = (*i).clone();
                    let i = i.into();
                    DBMessage {
                        cmd: crate::db::DBCommand::CopyImage { image: i, timestamp },
                        sender: x
                    } 
                },
                ClipboardEntry::Text ( t ) => {
                    DBMessage {
                        cmd: crate::db::DBCommand::CopyText { text: t.clone(), timestamp },
                        sender: x
                    } 
                }
            };
            tx.send(msg).await.expect("couldnt send msg");
            let _ = y.await.expect("failed to read response"); 
        }

        let mut updating_clock = self.clock.lock().expect("failed to acquire lock");
        for (key, value) in incoming_clock {
            let new_value = match updating_clock.get(key) {
                Some( old_value ) => {
                    if old_value > value {
                        *old_value
                    }
                    else {
                        *value
                    }
                }
                None => *value
            };
            let _ = updating_clock.insert(key.clone(), new_value);
        }
    }

    pub async fn listen(
        &self,
        mut rx: Receiver<ControlMessage>,
        mut tx: mpsc::Sender<DBMessage>,
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
                        println!("{:?}", neighbors[i]);
                        // no point in pinging if they are offline anyway
                        if !neighbors[i].Online {
                            continue
                        }
                        let ip = neighbors[i].TailscaleIPs[0].clone();
                        let endpoint = format!("http://{}:{}/clock", ip, PORT);
                        let incoming_clock = reqwest::get(endpoint)
                            .await
                            .expect("failed to send message")
                            .json::<HashMap<String, u64>>()
                            .await
                            .expect("failed to parse json");

                        // the incoming clock is newer
                        if self.is_outdated(&incoming_clock) {
                            // we must update our entries first, THEN our keys
                            let endpoint = format!("http://{}:{}/recent_clipboard", ip, PORT);
                            // TODO: actually parse / transmit data
                            let incoming_updates = reqwest::get(endpoint)
                                .await
                                .expect("failed to send message")
                                .json()
                                .await
                                .expect("failed to parse json");

                            self.update_values(&incoming_updates, &incoming_clock, &mut tx).await;
                        }
                    }
                    msg.sender.send(Ok(Response::OK)).expect("failed to reply");
                }
                ControlCommand::GetNeighbors => {
                    self.reload_neighbors().await;
                    let info = {
                        let n = self.neighbors.lock().expect("failed to acquire lock");
                        n.clone()
                    };
                    msg.sender.send(Ok(Response::Neighbors { info })).expect("failed to reply");
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
    GetNeighbors,
}

#[derive(Debug)]
pub enum Response {
    OK,
    Neighbors {info: Vec<PeerInfo>}
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
        sleep(duration).await;
    }
}
