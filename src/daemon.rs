use std::{fs, os::fd::AsRawFd, process::exit};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::{
    net::{UnixListener, UnixStream},
    task,
};
use ulid::Ulid;

use arboard;
use libc;

use crate::control_plane::{trigger_anti_entropy, ControlCommand, ControlMessage, Node};
use crate::db::{ClipboardWrapper, DBCommand, DBMessage, Database, Response};
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

    // db task
    let (database_tx, rx) = mpsc::channel(100);
    task::spawn(async move {
        let db = Database::new().expect("unable to create db");
        db.listen(rx).await;
    });

    // control plane task
    let (control_tx, rx) = mpsc::channel(100);
    let db_tx = database_tx.clone();
    task::spawn(async move {
        let node = Node::new().await;
        node.listen(rx, db_tx).await;
    });

    // anti entropy trigger
    let tx = control_tx.clone();
    task::spawn(async move {
        trigger_anti_entropy(tx).await;
    });

    // http task
    let db_tx_http = database_tx.clone();
    let c_tx_http = control_tx.clone();
    task::spawn(async move {
        run_http_server(db_tx_http, c_tx_http).await;
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
                let db_tx = database_tx.clone();
                let cp_tx = control_tx.clone();
                task::spawn(handle_client(stream, db_tx, cp_tx));
            }
            Err(e) => {
                eprintln!("connection failed: {}", e);
            }
        }
    }
}

async fn handle_client(
    mut stream: UnixStream,
    tx: mpsc::Sender<DBMessage>,
    cp_tx: mpsc::Sender<ControlMessage>,
) {
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
                cmd: DBCommand::Upload {
                    file_name: file_name.to_string(),
                    file_path: file_path.to_string(),
                    timestamp: Ulid::new(),
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
                cmd: DBCommand::Download {
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
                cmd: DBCommand::ListFiles,
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
            println!("got msg copy");
            let mut clipboard = arboard::Clipboard::new().expect("unable to open clipboard");
            let msg = {
                if let Ok(text) = clipboard.get_text() {
                    Some(DBMessage {
                        cmd: DBCommand::CopyText { text, timestamp: Ulid::new() },
                        sender: x,
                    })
                } else if let Ok(image) = clipboard.get_image() {
                    Some(DBMessage {
                        cmd: DBCommand::CopyImage { image: image.into(), timestamp: Ulid::new() },
                        sender: x,
                    })
                } else if let Ok(text) = fallback_get_clipboard_hyprland() {
                    Some(DBMessage {
                        cmd: DBCommand::CopyText { text, timestamp: Ulid::new() },
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
                        let (x, y) = oneshot::channel();
                        let msg = ControlMessage {
                            cmd: ControlCommand::Transmit {},
                            sender: x,
                        };
                        // doesnt matter if it fails to go through, we have anti entropy in place
                        let _ = cp_tx.send(msg).await;
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
                cmd: DBCommand::Paste {
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
                    cmd: DBCommand::History,
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

fn fallback_get_clipboard_hyprland() -> Result<String, ()> {
    println!("trying to read clipboard via wl-paste");
    use std::process::Command;
    let output = Command::new("wl-paste").arg("--no-newline").output().ok();

    if let Some(output) = output {
        if output.status.success() {
            println!("read from wl-paste");
            Ok(String::from_utf8(output.stdout).expect("failed to convert from utf8"))
        } else {
            println!("wl-paste failed");
            Err(())
        }
    } else {
        println!("wl-paste couldnt start?");
        Err(())
    }
}
