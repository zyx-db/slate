mod daemon;
mod db;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use daemon::start_daemon;
use daemon::stop_daemon;
use daemon::SOCKET_PATH;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "slate", about = "manage files and clipboards across devices")]
struct SlateCLI {
    #[command(subcommand)]
    command: SlateCommand,
}

#[derive(Subcommand, Debug)]
enum SlateCommand {
    /// copy data to the clipboard manager
    Copy,
    /// paste data from the clipboard manager
    Paste {
        offset: Option<usize>
    },
    /// upload a file
    Upload {
        /// file name for the upload
        filename: String,
        /// path to the desired upload file
        filepath: String,
    },
    /// show clipboard history
    History,
    /// list saved files
    Files,
    /// download file specified by name
    Download {
        /// name of the file to download
        filename: String,
        /// where you want the file downloaded
        filepath: Option<String>,
    },
    /// start the daemon service
    Start,
    /// stop the daemon service
    Stop,
    /// restart the daemon service
    Restart,
}

fn main() {
    let cli = SlateCLI::parse();
    println!("{:?}", cli);

    use SlateCommand::*;
    match cli.command {
        Start => {
            match start_daemon() {
                Err(e) => { eprintln!("{}", e)}
                Ok(_) => { println!("daemon started!")}
            };
        }
        Stop => {
            match stop_daemon() {
                Ok(_) => println!("daemon stopped"),
                Err(_) => println!("daemon was not running"),
            };
        }
        Restart => {
            let _ = stop_daemon();
            match start_daemon() {
                Ok(_) => println!("daemon restarted"),
                Err(_) => println!("unable to restart daemon")
            };
        }
        Copy => {
            send_command("copy");
        }
        Paste { offset } => {
            let offset = {
                match offset {
                    Some(x) => x,
                    None => 0
                }
            };
            send_command(&format!("paste {}", offset));
        }
        History => {
            send_command("history");
        }
        Files => {
            send_command("files");
        }
        Upload { filename, filepath } => {
            let pwd = std::env::current_dir().unwrap();
            let path = PathBuf::from(filepath);

            let final_path = pwd.join(path);
            let filepath = final_path.to_string_lossy();

            send_command(&format!("upload {} {}", filename, filepath));
        }
        Download { filename, filepath } => {
            let pwd = std::env::current_dir().unwrap();
            let filepath = {
                if let Some(filepath) = filepath {
                    let path = PathBuf::from(filepath);
                    pwd.join(path)
                } else {
                    pwd
                }
            };
            send_command(&format!(
                "download {} {}",
                filename,
                filepath.to_string_lossy()
            ));
        }
    }
}

fn send_command(command: &str) {
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
                    println!(
                        "response ({} files): {}",
                        formatted_files.len(),
                        formatted_files.join("\n")
                    );
                }
                _ => println!("response: {}", response.trim()),
            }
        }
        Err(_) => {
            eprintln!("daemon is not running");
        }
    }
}
