use serde::{Deserialize, Serialize};
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

use http::{header::HOST, Request};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use ulid::Ulid;

use crate::db::{ClipboardEntry, Clock, DBMessage};

const PORT: u64 = 3000;
const ANTI_ENTROPY_TIMEOUT_MS: u64 = 3 * 60 * 1000;
const TTL: u64 = 1;
const MAX_PER_ROUND: u64 = 5;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PeerInfo {
    HostName: String,
    TailscaleIPs: Vec<String>,
    Online: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Gossip {
    pub clock: Clock,
    pub entry: ClipboardEntry,
    pub ttl: u64,
}

pub fn is_outdated(clock: &Clock, incoming: &Clock) -> bool {
    incoming
        .iter()
        .any(|(key, &incoming_val)| match clock.get(key) {
            Some(&local_val) => incoming_val > local_val,
            None => true,
        })
}

pub struct Node {
    host_name: String,
    neighbors: Arc<Mutex<Vec<PeerInfo>>>,
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
            let body = res.collect().await.unwrap().to_bytes();
            let json_value: serde_json::Value =
                serde_json::from_slice(&body).expect("failed to parse json");

            // Extract just the "Peer" object
            let name_json = &json_value["Self"]["HostName"];
            serde_json::from_value(name_json.clone()).unwrap()
        };
        Node {
            host_name,
            neighbors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn get_clock(&self, tx: &mut mpsc::Sender<DBMessage>) -> Clock {
        let (x, y) = oneshot::channel();
        let msg = DBMessage {
            cmd: crate::db::DBCommand::LoadClock,
            sender: x,
        };
        tx.send(msg).await.expect("failed to send db msg");

        let response = y.await.expect("failed to recieve msg");

        if let Ok(crate::db::Response::Clock { data }) = response {
            data
        } else {
            eprintln!("{}", response.err().unwrap());
            HashMap::new()
        }
    }

    async fn save_clock(&self, clock: Clock, tx: &mut mpsc::Sender<DBMessage>) {
        let (x, y) = oneshot::channel();
        let msg = DBMessage {
            cmd: crate::db::DBCommand::SaveClock { clock },
            sender: x,
        };
        tx.send(msg).await.expect("failed to send db message");

        let _ = y.await;
    }

    async fn gossip(
        &self,
        entry: ClipboardEntry,
        neighbor_count: u64,
        ttl: u64,
        tx: &mut mpsc::Sender<DBMessage>,
    ) {
        self.reload_neighbors().await;
        let neighbors = {
            let n = self.neighbors.lock().expect("failed to acquire lock");
            n.clone()
        };
        let clock = self.get_clock(tx).await;
        let client = reqwest::Client::new();

        let mut sent = 0;
        for n in neighbors {
            if !n.Online {
                continue;
            };
            let ip = n.TailscaleIPs[0].clone();
            let endpoint = format!("http://{}:{}/gossip", ip, PORT);
            let clock = clock.clone();
            let entry = entry.clone();
            let body = Gossip { clock, ttl, entry };
            let _resp = client.post(endpoint).json(&body).send().await;

            // limit the number of messages
            sent += 1;
            if sent > neighbor_count {
                break;
            }
        }
    }

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
        let body = res.collect().await.unwrap().to_bytes();
        let json_value: serde_json::Value =
            serde_json::from_slice(&body).expect("failed to parse json");

        // Extract just the "Peer" object
        let peers_json = &json_value["Peer"];
        let peers: HashMap<String, PeerInfo> = serde_json::from_value(peers_json.clone()).unwrap();

        let neighbors: Vec<PeerInfo> = peers.into_values().collect();
        let mut cur = self.neighbors.lock().expect("failed to acquire lock");
        cur.clear();
        cur.extend(neighbors);
    }

    async fn is_outdated(&self, incoming: &Clock, tx: &mut mpsc::Sender<DBMessage>) -> bool {
        let clock = self.get_clock(tx).await;
        is_outdated(&clock, incoming)
    }

    async fn update_values(
        &self,
        incoming_updates: &Vec<(ClipboardEntry, String)>,
        incoming_clock: &Clock,
        tx: &mut mpsc::Sender<DBMessage>,
    ) {
        for update in incoming_updates {
            let (entry, timestamp) = update;
            let timestamp = Ulid::from_string(&timestamp).expect("failed to parse ulid");
            let (x, y) = oneshot::channel();
            let msg = match entry {
                ClipboardEntry::Image(i) => {
                    let i = (*i).clone();
                    let i = i.into();
                    DBMessage {
                        cmd: crate::db::DBCommand::CopyData {
                            data: ClipboardEntry::Image(i),
                            timestamp,
                            local: false,
                        },
                        sender: x,
                    }
                }
                ClipboardEntry::Text(t) => DBMessage {
                    cmd: crate::db::DBCommand::CopyData {
                        data: ClipboardEntry::Text(t.clone()),
                        timestamp,
                        local: false,
                    },
                    sender: x,
                },
            };
            tx.send(msg).await.expect("couldnt send msg");
            let _ = y.await.expect("failed to read response");
        }

        let mut updating_clock = self.get_clock(tx).await;
        println!("READING THE OLD CLOCK AS {:?}", updating_clock);
        for (key, value) in incoming_clock {
            let new_value = match updating_clock.get(key) {
                Some(old_value) => {
                    if old_value > value {
                        *old_value
                    } else {
                        *value
                    }
                }
                None => *value,
            };
            let _ = updating_clock.insert(key.clone(), new_value);
        }
        println!("SAVING THE NEW CLOCK AS {:?}", updating_clock);
        self.save_clock(updating_clock, tx).await;
    }

    pub async fn listen(&self, mut rx: Receiver<ControlMessage>, mut tx: mpsc::Sender<DBMessage>) {
        println!("control plane started!");

        // init row, if needed
        {
            let (x, y) = oneshot::channel();
            let msg = DBMessage {
                cmd: crate::db::DBCommand::InsertSelf {
                    host_name: self.host_name.clone(),
                },
                sender: x,
            };
            tx.send(msg).await.expect("failed to init row");
            let status = y.await.expect("failed to recieve msg");
            status.expect("did not create self row?");
        }

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

                    let client = reqwest::Client::new();

                    for i in 0..neighbors.len() {
                        // no point in pinging if they are offline anyway
                        if !neighbors[i].Online {
                            continue;
                        }
                        let ip = neighbors[i].TailscaleIPs[0].clone();
                        let endpoint = format!("http://{}:{}/clock", ip, PORT);
                        let incoming_clock = match client.get(&endpoint).send().await {
                            Ok(response) => match response.json::<Clock>().await {
                                Ok(clock) => clock,
                                Err(e) => {
                                    eprintln!("Failed to parse JSON from {}: {}", endpoint, e);
                                    continue;
                                }
                            },
                            Err(e) => {
                                eprintln!("Failed to send request to {}: {}", endpoint, e);
                                continue;
                            }
                        };

                        // the incoming clock is newer
                        if self.is_outdated(&incoming_clock, &mut tx).await {
                            // we must update our entries first, THEN our keys
                            let endpoint = format!("http://{}:{}/recent_clipboard", ip, PORT);
                            let incoming_updates = client
                                .get(endpoint)
                                .send()
                                .await
                                .expect("failed to send message")
                                .json()
                                .await
                                .expect("failed to parse json");

                            self.update_values(&incoming_updates, &incoming_clock, &mut tx)
                                .await;
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
                    msg.sender
                        .send(Ok(Response::Neighbors { info }))
                        .expect("failed to reply");
                }
                ControlCommand::GetClock => {
                    let data = self.get_clock(&mut tx).await;
                    msg.sender
                        .send(Ok(Response::Clock { data }))
                        .expect("failed to reply");
                }
                ControlCommand::Transmit { data, ttl, clock } => {
                    let successfully_saved = {
                        let (x, y) = oneshot::channel();
                        let msg = DBMessage {
                            cmd: crate::db::DBCommand::CopyData {
                                data: data.clone(),
                                timestamp: Ulid::new(),
                                local: clock.is_none(),
                            },
                            sender: x,
                        };
                        tx.send(msg).await.expect("failed to msg db");
                        let resp = y.await.expect("failed to read response");
                        resp.is_ok()
                    };

                    if successfully_saved {

                        if clock.is_some() {
                            self.save_clock(clock.unwrap(), &mut tx);
                        };

                        let ttl = match ttl {
                            Some(x) => x,
                            None => TTL,
                        };
                        self.gossip(data, MAX_PER_ROUND, ttl, &mut tx).await;
                        msg.sender.send(Ok(Response::OK)).expect("failed to reply");
                    } else {
                        msg.sender
                            .send(Err("failed to save".into()))
                            .expect("failed to reply");
                    }
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
    Transmit {
        data: ClipboardEntry,
        ttl: Option<u64>,
        clock: Option<Clock>
    },
    GetNeighbors,
    GetClock,
}

#[derive(Debug)]
pub enum Response {
    OK,
    Neighbors { info: Vec<PeerInfo> },
    Clock { data: Clock },
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
