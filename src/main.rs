mod daemon;
mod db;

use std::path::PathBuf;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

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
    Paste,
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
    Download,
    /// start the daemon service
    Start,
    /// stop the daemon service
    Stop,
}

fn main() {
    let cli = SlateCLI::parse();
    println!("{:?}", cli);

    use SlateCommand::*;
    match cli.command {
        Start => {start_daemon();}
        Stop => {stop_daemon();}
        Copy => {send_command("copy");}
        Paste => {send_command("paste");}
        History => {send_command("history");}
        Files => {send_command("files");}
        Upload{ filename, filepath } => {
            let pwd = std::env::current_dir().unwrap();
            let path = PathBuf::from(filepath);

            let final_path = pwd.join(path);
            let filepath = final_path.to_string_lossy();

            send_command(&format!("upload {} {}", filename, filepath));
        }
        _ => {}
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
