use arboard::ImageData;
use rusqlite::{params, Connection};
use std::fmt::Debug;
use std::{fs, io::Read};
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot::Sender;
use zstd::stream::encode_all;

const DATABASE_PATH: &str = "/tmp/slate_daemon.sqlite";

pub struct Database {
    connection: Connection,
}

enum ClipboardEntry <'a> {
    Image(ImageData<'a>),
    Text(String) 
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
            CREATE TABLE IF NOT EXISTS clipboard (
                key INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                text_data TEXT,
                width INTEGER,
                height INTEGER,
                image_content BLOB 
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
            INSERT INTO clipboard (text_data) VALUES (?1)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![text])
    }

    fn save_image(&self, image: ImageData) -> Result<usize, rusqlite::Error>{
        let query = "
            INSERT INTO clipboard (width, height, image_content) VALUES (?1, ?2, ?3)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![image.width, image.height, image.bytes])
    }

    fn read_clipboard(&self, offset: usize) -> Result<ClipboardEntry, rusqlite::Error> {
        let query = "
            SELECT c.text_data, c.width, c.height, c.image_content
            FROM clipboard c
            ORDER BY key DESC
            LIMIT 1 OFFSET ?;
        ";

        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.query_row(params![offset], |row| {
            let text: Option<String> = row.get::<usize, Option<String>>(0)?;
            let width: Option<usize> = row.get::<usize, Option<usize>>(1)?;
            let height: Option<usize> = row.get::<usize, Option<usize>>(2)?;
            let content: Option<Vec<u8>> = row.get::<usize, Option<Vec<u8>>>(3)?;

            println!("{:?} {:?} {:?} {:?}", text, width, height, &content);
            if let Some(t) = text {
                return Ok(ClipboardEntry::Text(t));
            } else if let (Some(w), Some(h), Some(img)) = (width, height, &content) {
                return Ok(ClipboardEntry::Image(
                    ImageData { width: w, height: h, bytes: std::borrow::Cow::Owned(img.clone()) }
                ));
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
    }

    pub async fn listen(self, mut rx: Receiver<DBMessage<'_>>) {
        println!("db started!");
        while let Some(msg) = rx.recv().await {
            let tx = msg.sender;
            let cmd = msg.cmd;
            use Command::*;
            match cmd {
                Upload {
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
                ListFiles => {
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
                CopyImage {
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
                CopyText {
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
                Paste { offset, mut clipboard } => {
                    let result = self.read_clipboard(offset);
                    let mut completed = true;
                    if result.is_ok() {
                        let r = result.unwrap();
                        use ClipboardEntry::*;
                        match r {
                            Image( i ) => {
                                if (clipboard.inner.set_image(i)).is_err() {
                                    println!("failed to set image");
                                    completed = false;
                                }
                            }
                            Text( t ) => {
                                if (clipboard.inner.set_text(t)).is_err() {
                                    println!("failed to set text");
                                    completed = false;
                                }
                            }
                        };
                    } else {
                        println!("failed to read db");
                        completed = false;
                    }

                    if completed {
                        tx.send(Ok(Response::PasteSuccessful)).expect("failed to send response");
                    } else {
                        tx.send(Err("failed to paste".to_string())).expect("failed to send response");
                    }
                }
                _ => {}
            }
        }
    }
}

pub struct ClipboardWrapper {
    pub inner: arboard::Clipboard
}

impl Debug for ClipboardWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clipboard")
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
    Paste {
        offset: usize,
        clipboard: ClipboardWrapper
    },
    ListFiles,
}

#[derive(Debug)]
pub enum Response {
    UploadSuccessful,
    CopySuccessful,
    PasteSuccessful,
    Files { names: Vec<String> },
}

#[derive(Debug)]
pub struct DBMessage<'a> {
    pub cmd: Command<'a>,
    pub sender: Sender<Result<Response, String>>,
}
