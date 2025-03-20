use std::{fs, os::fd::AsRawFd, process::exit};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    net::{UnixListener, UnixStream},
    task,
};

use libc;

use crate::db::{Command, DBMessage, Database, Response};

pub const SOCKET_PATH: &str = "/tmp/slate_daemon.sock";
const PID_FILE: &str = "/tmp/slate_daemon.pid";

pub fn start_daemon() {
    if let Ok(_) = fs::metadata(PID_FILE) {
        eprintln!("slate daemon is already running!");
        exit(1);
    }

    // fork proc
    match unsafe { libc::fork() } {
        -1 => {
            eprintln!("failed to fork process to start daemon");
            exit(1);
        }
        0 => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            if let Err(e) = rt.block_on(run_daemon()) {
                eprintln!("daemon error: {}", e);
                exit(1);
            }
        }
        _ => {
            println!("slate daemon started!")
        }
    }
}

async fn run_daemon() -> std::io::Result<()> {
    // output prints to a log file, easy to debug
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/slate_daemon.log")?;

    let stdout = log_file.try_clone()?;
    let stderr = log_file.try_clone()?;
    unsafe {
        libc::dup2(stdout.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(stderr.as_raw_fd(), libc::STDERR_FILENO);
    }

    println!("started service");

    let (tx, rx) = mpsc::channel(100);

    // db task
    task::spawn(async move {
        let db = Database::new().expect("unable to create db");
        db.listen(rx).await;
    });

    // http task
    let http_sender = tx.clone();
    task::spawn(async move {
        //let http_server = HTTPServer::new();
        //http_server.listen(http_sender);
    });

    // create PID file and a SOCKET file for daemon
    fs::write(PID_FILE, std::process::id().to_string())?;

    if fs::metadata(SOCKET_PATH).is_ok() {
        fs::remove_file(SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = tx.clone();
                task::spawn(handle_client(stream, tx));
            }
            Err(e) => {
                eprintln!("connection failed: {}", e);
            }
        }
    }
}

async fn handle_client(mut stream: UnixStream, tx: mpsc::Sender<DBMessage>) {
    let mut reader = BufReader::new(&mut stream);
    let mut command = String::new();

    if reader.read_line(&mut command).await.is_err() {
        eprintln!("failed to read command");
        return;
    }

    let command = command.trim();
    println!("got command {}", command);

    let (x, y) = oneshot::channel();
    let response = match command {
        cmd if cmd.starts_with("upload ") => {
            let args = cmd.strip_prefix("upload ").unwrap().to_string();
            let (file_name, file_path) = args.split_once(" ").unwrap();

            let msg = DBMessage {
                cmd: Command::Upload {
                    file_name: file_name.to_string(),
                    file_path: file_path.to_string(),
                },
                sender: x,
            };

            if let Err(e) = tx.send(msg).await {
                format!("unable to send msg to db {}", e)
            } else {
                let response = y.await.expect("failed to read response");
                match response {
                    Ok(_) => format!("uploading file {} from {}\n", file_name, file_path),
                    Err(e) => format!(
                        "uploading file {} from {} got error {}\n",
                        file_name, file_path, e
                    ),
                }
            }
        }
        "files" => {
            let msg = DBMessage {
                cmd: Command::ListFiles,
                sender: x,
            };

            if let Err(e) = tx.send(msg).await {
                format!("unable to send msg to db {}", e)
            } else {
                let response = y.await.expect("failed to read response");
                match response {
                    Ok(Response::Files { names }) => {
                        if names.len() == 0 {
                            "NO FILES".to_string()
                        } else {
                            format!("slate_files {}\n", names.join(" "))
                        }
                    }

                    Err(e) => format!("listing files got error {}\n", e),
                    _ => {
                        format!("SHOULD NEVER PRINT?!\n")
                    }
                }
            }
        }
        _ => format!("hey {}\n", command),
    };

    if let Err(e) = reader.get_mut().write_all(response.as_bytes()).await {
        eprintln!("failed to send response: {}", e);
    }
}

pub fn stop_daemon() {
    if let Ok(pid) = fs::read_to_string(PID_FILE) {
        let pid: i32 = pid.trim().parse().unwrap();
        unsafe { libc::kill(pid, libc::SIGTERM) };
        fs::remove_file(PID_FILE).unwrap();
        fs::remove_file(SOCKET_PATH).unwrap();
        println!("daemon stopped");
    } else {
        println!("daemon was not running");
    }
}
