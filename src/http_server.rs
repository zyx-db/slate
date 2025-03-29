use std::{sync::{Arc, Mutex}, thread::sleep, time::Duration};
use axum::{
    routing::get,
    Router
};

const REFRESH_NEIGHBORS_TIMEOUT: u64 = 10 * 60 * 1000;
const ANTI_ENTROPY_TIMEOUT_MS: u64 = 1 * 60 * 1000;

async fn health_check() -> &'static str {
    "hai"
}

struct Node {
    neighbors: Arc<Mutex<Vec<String>>>,
}

impl Node {
    fn new() -> Self {
        Node { neighbors: Arc::new(Mutex::new(Vec::new())) }
    }

    async fn background_events(&self){
        let mut iterations = 0;
        let timeout = Duration::from_millis(ANTI_ENTROPY_TIMEOUT_MS);
        let poll_neighbors = REFRESH_NEIGHBORS_TIMEOUT / ANTI_ENTROPY_TIMEOUT_MS;
        loop {
            sleep(timeout);
            // TODO: anti entropy messages
            if iterations == poll_neighbors {
                let mut neighbors = self.neighbors.lock().unwrap();
                neighbors.clear();
                // TODO: poll neighbors
                iterations = 0;
            } else {
                iterations += 1;
            }
        }
    }

    async fn get_clock(&self){}

    async fn read(&self){}

    async fn gossip(&self){}

    async fn listen(&self) {
        loop {}
    }
}

pub async fn run_http_server() {
    // background task for
    // anti entropy
    // refreshing neighbors
    tokio::spawn(async move {
        let node = Node::new();
        node.listen().await;
    });

    let app = Router::new()
        //.nest()
        .route("/health", get(health_check));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
} 
