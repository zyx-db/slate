use std::{
    fs,
    io::{BufRead,BufReader, Write},
    os::{fd::AsRawFd, unix::net::{UnixListener, UnixStream}},
    process::{exit, Command},
    thread,
};
use libc;
use rusqlite::params;

use crate::db::{self, Database};

const SOCKET_PATH: &str = "/tmp/slate_daemon.sock";
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
            if let Err(e) = run_daemon() {
                eprintln!("daemon error: {}", e);
                exit(1);
            }
        }
        _ => {
            println!("stale daemon started!")
        }
    }
}

fn run_daemon() -> std::io::Result<()> {
    // output prints to a log file, easy to debug
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/slate_daemon.log")?;

    let stdout = log_file.try_clone()?;
    let stderr = log_file.try_clone()?; unsafe {
        libc::dup2(stdout.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(stdout.as_raw_fd(), libc::STDERR_FILENO);
    }

    println!("started service");

    // create PID file and a SOCKET file for daemon
    fs::write(PID_FILE, std::process::id().to_string())?;

    if fs::metadata(SOCKET_PATH).is_ok() {
        fs::remove_file(SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_client(stream));
            }
            Err(e) => eprintln!("connection failed: {}", e)
        }
    }

    Ok(())
}

fn handle_client(mut stream: UnixStream) {
    let mut reader = BufReader::new(&stream);
    let mut command = String::new();
    reader.read_line(&mut command).unwrap();
    let command = command.trim();

    println!("got command {}", command);

    if let Ok(db) = Database::new() {
        let response = match command {
            cmd if cmd.starts_with("upload ") => {
                let args = cmd.strip_prefix("upload ").unwrap().to_string();
                let (file_name, file_path) = args.split_once(" ").unwrap();
                match db.upload_file(file_name, file_path) {
                    Ok(_) => format!("uploading file {} from {}", file_name, file_path),
                    Err(e) => format!("uploading file {} from {} got error {}", file_name, file_path, e)
                }
            }
            "files" => {
                let file_names = db.get_files().unwrap();
                println!("read files {:?}", file_names);
                match file_names.len() {
                    0 => "NO FILES".to_string(),
                    _ => format!("slate_files {}", file_names.join(" "))
                }
            }
            _ => format!("hey {}", command)
        };

        writeln!(stream, "{}", response).unwrap();
    } else {
        writeln!(stream, "unable to open db").unwrap();
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

pub fn send_command(command: &str) {
    match UnixStream::connect(SOCKET_PATH) {
        Ok(mut stream) => {
            let write = writeln!(stream, "{}", command);
            if write.is_err() {
                eprintln!("failed to send msg");
                return;
            }
            
            let mut response = String::new();
            let read = BufReader::new(stream).read_line(&mut response);
            if read.is_err() {
                eprintln!("failed to read response");
                return;
            }
            match response {
                r if r.starts_with("slate_files ") => {
                    let response = r.strip_prefix("slate_files ").unwrap();
                    let formatted_files = response
                        .split(" ")
                        .map(|s| s.to_string())
                        .collect::<Vec<String>>();
                    println!("response ({} files): {}", formatted_files.len(), formatted_files.join("\n"));
                }
                _ => println!("response: {}", response.trim())
            }
        }
        Err(_) => {
            eprintln!("daemon is not running");
        }
    }
}
