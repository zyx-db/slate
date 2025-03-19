use std::{
    fs,
    os::fd::AsRawFd,
    process::exit,
};

use tokio::{net::{UnixListener, UnixStream}, task};
use tokio::io::{BufReader, AsyncBufReadExt, AsyncWriteExt};

use libc;

use crate::db::Database;

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
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
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
    let stderr = log_file.try_clone()?; unsafe {
        libc::dup2(stdout.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(stderr.as_raw_fd(), libc::STDERR_FILENO);
    }

    println!("started service");

    // create PID file and a SOCKET file for daemon
    fs::write(PID_FILE, std::process::id().to_string())?;

    if fs::metadata(SOCKET_PATH).is_ok() {
        fs::remove_file(SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                task::spawn(handle_client(stream));
            }
            Err(e) => {
                eprintln!("connection failed: {}", e);
            }
        }
    }
}

async fn handle_client(mut stream: UnixStream){
    let mut reader = BufReader::new(&mut stream);
    let mut command = String::new();

    if reader.read_line(&mut command).await.is_err() {
        eprintln!("failed to read command");
        return;
    }

    let command = command.trim();
    println!("got command {}", command);

    if let Ok(db) = Database::new() {
        let response = match command {
            cmd if cmd.starts_with("upload ") => {
                let args = cmd.strip_prefix("upload ").unwrap().to_string();
                let (file_name, file_path) = args.split_once(" ").unwrap();
                match db.upload_file(file_name, file_path) {
                    Ok(_) => format!("uploading file {} from {}\n", file_name, file_path),
                    Err(e) => format!("uploading file {} from {} got error {}\n", file_name, file_path, e)
                }
            }
            "files" => {
                let file_names = db.get_files().unwrap();
                println!("read files {:?}", file_names);
                match file_names.len() {
                    0 => "NO FILES".to_string(),
                    _ => format!("slate_files {}\n", file_names.join(" "))
                }
            }
            _ => format!("hey {}\n", command)
        };

        if let Err(e) = reader.get_mut().write_all(response.as_bytes()).await {
            eprintln!("failed to send response: {}", e);
        }
    } else {
        eprintln!("unable to open db");
    }
}

pub fn stop_daemon(){
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
