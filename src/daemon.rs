use std::{fs, os::fd::AsRawFd, process::exit};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    net::{UnixListener, UnixStream},
    task,
};

use arboard;
use libc;

use crate::db::{ClipboardWrapper, Command, DBMessage, Database, Response};
use crate::http_server::run_http_server;

pub const SOCKET_PATH: &str = "/tmp/slate_daemon.sock";
const PID_FILE: &str = "/tmp/slate_daemon.pid";

pub fn start_daemon() -> Result<(), String> {
    if let Ok(_) = fs::metadata(PID_FILE) {
        eprintln!("slate daemon is already running!");
        exit(1);
    }

    // fork proc
    match unsafe { libc::fork() } {
        -1 => Err(format!("failed to fork process to start daemon")),
        0 => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            if let Err(e) = rt.block_on(run_daemon()) {
                Err(format!("daemon error: {}", e))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
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

    // control plane task
    //task::spawn(async move {
    //    let node = Node::new();
    //    node.listen(control_rx);
    //});

    // http task
    let http_sender = tx.clone();
    task::spawn(async move {
        run_http_server().await;
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

async fn handle_client(mut stream: UnixStream, tx: mpsc::Sender<DBMessage<'_>>) {
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
        cmd if cmd.starts_with("download ") => {
            let args = cmd.strip_prefix("download ").unwrap().to_string();
            let (file_name, file_path) = args.split_once(" ").unwrap();

            let msg = DBMessage {
                cmd: Command::Download {
                    download_path: file_path.to_string(),
                    file_name: file_name.to_string(),
                },
                sender: x,
            };
            if let Err(e) = tx.send(msg).await {
                format!("unable to send msg to db {}", e)
            } else {
                let response = y.await.expect("failed to read response");
                match response {
                    Ok(_) => format!("downloading file {} at {}\n", file_name, file_path),
                    Err(e) => format!(
                        "downloading file {} at {} got error {}\n",
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
        "copy" => {
            let mut clipboard = arboard::Clipboard::new().expect("unable to open clipboard");
            let msg = {
                if let Ok(text) = clipboard.get_text() {
                    Some(DBMessage {
                        cmd: Command::CopyText { text },
                        sender: x,
                    })
                } else if let Ok(image) = clipboard.get_image() {
                    Some(DBMessage {
                        cmd: Command::CopyImage { image },
                        sender: x,
                    })
                } else {
                    eprintln!("failed to get text: {}", clipboard.get_text().unwrap_err());
                    None
                }
            };

            if msg.is_none() {
                format!("failed to copy")
            } else if let Err(e) = tx.send(msg.unwrap()).await {
                format!("unable to send message to db {}", e)
            } else {
                let response = y.await.expect("failed to read response");
                match response {
                    Ok(_) => {
                        format!("successfully copied to db")
                    }
                    Err(e) => {
                        format!("error copying to db: {}", e)
                    }
                }
            }
        }
        cmd if cmd.starts_with("paste ") => {
            let cmd = command.strip_prefix("paste ").unwrap();
            let offset = cmd.parse::<usize>().unwrap();
            let clipboard = arboard::Clipboard::new().expect("unable to open clipboard");
            let msg = DBMessage {
                cmd: Command::Paste {
                    offset,
                    clipboard: ClipboardWrapper { inner: clipboard },
                },
                sender: x,
            };

            if let Err(e) = tx.send(msg).await {
                format!("unable to send message to db {}", e)
            } else {
                let response = y.await.expect("failed to read response");
                match response {
                    Ok(_) => {
                        format!("successfully pasted to clipboard")
                    }
                    Err(e) => {
                        format!("error pasting to clipboard: {}", e)
                    }
                }
            }
        }
        "history" => {
            if tx
                .send(DBMessage {
                    cmd: Command::History,
                    sender: x,
                })
                .await
                .is_err()
            {
                format!("failed to send message to db")
            } else {
                match y.await.expect("failed to read response") {
                    Ok(Response::History { names }) => {
                        format!("history {}", names.join(" "))
                    }
                    Err(e) => format!("error getting history {}", e),
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

pub fn stop_daemon() -> Result<(), ()> {
    if let Ok(pid) = fs::read_to_string(PID_FILE) {
        let pid: i32 = pid.trim().parse().unwrap();
        unsafe { libc::kill(pid, libc::SIGTERM) };
        fs::remove_file(PID_FILE).unwrap();
        fs::remove_file(SOCKET_PATH).unwrap();
        Ok(())
    } else {
        Err(())
    }
}
