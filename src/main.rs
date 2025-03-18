mod daemon;

use daemon::send_command;
use daemon::start_daemon;
use daemon::stop_daemon;

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
        filepath: String,
    },
    /// show clipboard history
    History,
    /// list saved files
    Files,
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
        Upload{ filepath } => {
        }
        _ => {}
    }
}
