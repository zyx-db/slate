use std::{
    fs,
    io::{BufRead,BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::{exit, Command},
    thread,
};
use libc;

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

    let response = match command {
        cmd if cmd.starts_with("upload ") => {
            let file_path = cmd.strip_prefix("upload").unwrap();
            format!("uploading file at {}", file_path)
        }
        _ => format!("hey {}", command)
    };

    writeln!(stream, "{}", response).unwrap();
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
            writeln!(stream, "{}", command).unwrap();

            let mut response = String::new();
            BufReader::new(stream).read_line(&mut response).unwrap();

            println!("{}", response.trim());
        }
        Err(_) => {
            eprintln!("daemon is not running");
        }
    }
}
