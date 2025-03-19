mod daemon;
mod db;

use std::path::PathBuf;

use daemon::send_command;
use daemon::start_daemon;
use daemon::stop_daemon;

use clap::{Parser, Subcommand};
use db::Database;

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
    let db = Database::new().unwrap();

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
