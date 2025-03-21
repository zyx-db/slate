use arboard::ImageData;
use rusqlite::{params, Connection};
use std::{fs, io::Read};
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot::Sender;
use zstd::stream::encode_all;

const DATABASE_PATH: &str = "/tmp/slate_daemon.sqlite";

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn new() -> Result<Self, rusqlite::Error> {
        let connection = Connection::open(DATABASE_PATH)?;
        //let connection = Connection::open_in_memory()?;
        let sql = "
            CREATE TABLE IF NOT EXISTS files (
                key INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                file_name TEXT UNIQUE NOT NULL,
                content BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS images (
                key INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                content BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS clipboard_text (
                key INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL
            );
        ";

        connection.execute_batch(sql)?;

        Ok(Database { connection })
    }

    fn upload_file(&self, filename: &str, filepath: &str) -> Result<(), String> {
        println!("opening file from {} with name {}", filepath, filename);
        let mut file = fs::File::open(filepath).expect("cannot open file");
        let mut file_data = Vec::new();
        file.read_to_end(&mut file_data)
            .expect("failed to read file");

        let compressed_data = encode_all(&file_data[..], 3).unwrap();
        self.connection
            .execute(
                "INSERT INTO files (file_name, content) VALUES (?1, ?2)",
                params![filename, compressed_data],
            )
            .unwrap();

        Ok(())
    }

    fn get_files(&self) -> Result<Vec<String>, rusqlite::Error> {
        let query = "
        SELECT f.file_name
        FROM files f;
        ";

        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        let res: Result<Vec<String>, rusqlite::Error> = statement
            .query_map([], |row| row.get::<usize, String>(0))?
            .collect();

        match res {
            Ok(res) => return Ok(res),
            Err(e) => return Err(e),
        }
    }

    fn save_text(&self, text: String) -> Result<usize, rusqlite::Error>{
        let query = "
            INSERT INTO clipboard_text (content) VALUES (?1)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![text])
    }

    fn save_image(&self, image: ImageData) -> Result<usize, rusqlite::Error>{
        let query = "
            INSERT INTO images (width, height, content) VALUES (?1, ?2, ?3)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![image.width, image.height, image.bytes])
    }

    pub async fn listen(self, mut rx: Receiver<DBMessage<'_>>) {
        while let Some(msg) = rx.recv().await {
            let tx = msg.sender;
            let cmd = msg.cmd;
            match cmd {
                Command::Upload {
                    file_name,
                    file_path,
                } => {
                    let result = self.upload_file(&file_name, &file_path);
                    match result {
                        Ok(()) => {
                            tx.send(Ok(Response::UploadSuccessful))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e)).expect("failed to send response");
                        }
                    }
                }
                Command::ListFiles => {
                    let result = self.get_files();
                    match result {
                        Ok(x) => {
                            tx.send(Ok(Response::Files { names: x }))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
                        }
                    }
                }
                Command::CopyImage {
                    image
                } => {
                    let result = self.save_image(image);
                    match result {
                        Ok(_) => {
                            tx.send(Ok(Response::CopySuccessful))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
                        }
                    }
                }
                Command::CopyText {
                    text
                } => {
                    let result = self.save_text(text);
                    match result {
                        Ok(_) => {
                            tx.send(Ok(Response::CopySuccessful))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Debug)]
pub enum Command<'a> {
    Upload {
        file_name: String,
        file_path: String,
    },
    Download {
        download_path: String,
        file_name: String,
    },
    CopyImage {
        image: ImageData<'a>
    },
    CopyText {
        text: String
    },
    ListFiles,
}

#[derive(Debug)]
pub enum Response {
    UploadSuccessful,
    CopySuccessful,
    Files { names: Vec<String> },
}

#[derive(Debug)]
pub struct DBMessage<'a> {
    pub cmd: Command<'a>,
    pub sender: Sender<Result<Response, String>>,
}
