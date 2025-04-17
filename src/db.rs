use arboard::ImageData;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Debug;
use std::{fs, io::Read};
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot::Sender;
use ulid::Ulid;
use zstd::stream::encode_all;

const DATABASE_PATH: &str = "/tmp/slate_daemon.sqlite";

pub struct Database {
    connection: Connection,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SerializableImage {
    width: usize,
    height: usize,
    bytes: Vec<u8>, // owned!
}

impl<'a> From<ImageData<'a>> for SerializableImage {
    fn from(img: ImageData<'a>) -> Self {
        Self {
            width: img.width,
            height: img.height,
            bytes: img.bytes.to_vec(),
        }
    }
}

impl<'a> Into<ImageData<'a>> for SerializableImage {
    fn into(self) -> ImageData<'a> {
        ImageData {
            width: self.width,
            height: self.height,
            bytes: Cow::Owned(self.bytes),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ClipboardEntry {
    Image(SerializableImage),
    Text(String),
}

impl Database {
    pub fn new() -> Result<Self, rusqlite::Error> {
        let connection = Connection::open(DATABASE_PATH)?;
        //let connection = Connection::open_in_memory()?;
        let sql = "
            CREATE TABLE IF NOT EXISTS files (
                key INTEGER NOT NULL PRIMARY KEY,
                file_name TEXT UNIQUE NOT NULL,
                content BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS clipboard (
                -- using ULID for key, can sort by time, while unique across nodes
                key TEXT NOT NULL PRIMARY KEY,
                text_data TEXT,
                width INTEGER,
                height INTEGER,
                image_content BLOB 
            );
            CREATE TABLE IF NOT EXISTS clock (
                key TEXT NOT NULL PRIMARY KEY,
                self BOOLEAN NOT NULL,
                time INTEGER NOT NULL
            )
        ";

        connection.execute_batch(sql)?;

        Ok(Database { connection })
    }

    fn sync_clock(&self, clock_map: &HashMap<String, u64>) -> Result<(), rusqlite::Error> {
        if clock_map.is_empty() {
            return Ok(());
        }

        // Create the parameterized query for non-self entries only
        let placeholders: Vec<_> = (0..clock_map.len())
            .map(|i| format!("(?{}, FALSE, ?{})", i * 2 + 1, i * 2 + 2))
            .collect();

        let sql = format!(
            "INSERT INTO clock (key, self, time) VALUES {} 
             ON CONFLICT(key) DO UPDATE SET time = excluded.time 
             WHERE self = FALSE", // Only update non-self entries
            placeholders.join(",")
        );

        // Convert HashMap entries to parameters (excluding self entries)
        let params: Vec<_> = clock_map
            .iter()
            .flat_map(|(k, v)| vec![k as &dyn rusqlite::ToSql, v as &dyn rusqlite::ToSql])
            .collect();

        self.connection.execute(&sql, &params[..])?;
        Ok(())
    }

    fn load_clock(&self) -> Result<HashMap<String, u64>, rusqlite::Error> {
        let mut stmt = self.connection.prepare("SELECT key, time FROM clock")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;

        let mut clock_map = HashMap::new();
        for row in rows {
            let (key, time) = row?;
            clock_map.insert(key, time);
        }
        Ok(clock_map)
    }

    fn inc_self_counter(&self) -> Result<(), rusqlite::Error> {
        let sql = "UPDATE clock SET time = time + 1 WHERE self = TRUE";
        self.connection.execute(sql, [])?;
        Ok(())
    }

    fn upload_file(
        &self,
        filename: &str,
        filepath: &str,
        timestamp: Ulid,
        local: bool
    ) -> Result<(), rusqlite::Error> {
        if local {
            self.inc_self_counter()?;
        }
        println!("opening file from {} with name {}", filepath, filename);
        let mut file = fs::File::open(filepath).expect("cannot open file");
        let mut file_data = Vec::new();
        file.read_to_end(&mut file_data)
            .expect("failed to read file");

        let compressed_data = encode_all(&file_data[..], 3).unwrap();
        self.connection
            .execute(
                "INSERT INTO files (key, file_name, content) VALUES (?1, ?2)",
                params![timestamp.to_string(), filename, compressed_data],
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

    fn get_history(&self) -> Result<Vec<String>, rusqlite::Error> {
        let query = "
            SELECT c.text_data
            FROM clipboard c
            ORDER BY key DESC
            LIMIT 20;
        ";

        let mut statement = self
            .connection
            .prepare(query)
            .expect("failed to prepare query");

        let result = statement
            .query_map(params![], |row| {
                let name: Option<String> = row.get::<usize, Option<String>>(0)?;
                Ok(name.unwrap_or_else(|| "image".to_string()))
            })?
            .collect::<Result<Vec<String>, rusqlite::Error>>();

        result
    }

    fn save_text(&self, text: String, timestamp: Ulid, local: bool) -> Result<usize, rusqlite::Error> {
        if local {
            self.inc_self_counter()?;
        }
        let query = "
            INSERT INTO clipboard (key, text_data) VALUES (?1, ?2)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![timestamp.to_string(), text])
    }

    fn save_image(
        &self,
        image: SerializableImage,
        timestamp: Ulid,
        local: bool,
    ) -> Result<usize, rusqlite::Error> {
        if local {
            self.inc_self_counter()?;
        }
        let query = "
            INSERT INTO clipboard (key, width, height, image_content) VALUES (?1, ?2, ?3, ?4)
        ";
        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        statement.execute(params![
            timestamp.to_string(),
            image.width,
            image.height,
            image.bytes
        ])
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
                return Ok(ClipboardEntry::Image(SerializableImage {
                    width: w,
                    height: h,
                    bytes: img.clone(),
                }));
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
    }

    pub fn get_recent(&self, limit: u64) -> Result<Vec<(ClipboardEntry, String)>, rusqlite::Error> {
        let query = "
            SELECT c.key, c.text_data, c.width, c.height, c.image_content
            FROM clipboard c
            ORDER BY c.key DESC
            LIMIT ?;
        ";

        let mut statement = self
            .connection
            .prepare(query)
            .expect("unable to prepare query");

        let rows = statement.query_map(params![limit], |row| {
            let key: String = row.get(0)?;
            let text: Option<String> = row.get(1)?;
            let width: Option<usize> = row.get(2)?;
            let height: Option<usize> = row.get(3)?;
            let content: Option<Vec<u8>> = row.get(4)?;

            let entry = if let Some(t) = text {
                ClipboardEntry::Text(t)
            } else if let (Some(w), Some(h), Some(img)) = (width, height, content) {
                ClipboardEntry::Image(SerializableImage {
                    width: w,
                    height: h,
                    bytes: img,
                })
            } else {
                // Gracefully skip invalid row
                return Err(rusqlite::Error::InvalidQuery);
            };

            Ok((entry, key))
        })?;

        // Collecting into Vec
        rows.collect()
    }

    pub fn insert_self(&self, host_name: String) -> Result<(), rusqlite::Error> {
        let sql = "
            INSERT INTO clock (key, self, time) VALUES (?1, TRUE, 0)
            ON CONFLICT(key) DO NOTHING
        ";

        self.connection.execute(&sql, params![host_name])?;
        Ok(())
    }

    pub async fn listen(self, mut rx: Receiver<DBMessage>) {
        println!("db started!");
        while let Some(msg) = rx.recv().await {
            let tx = msg.sender;
            let cmd = msg.cmd;
            use DBCommand::*;
            match cmd {
                Upload {
                    file_name,
                    file_path,
                    timestamp,
                    local,
                } => {
                    let result = self.upload_file(&file_name, &file_path, timestamp, local);
                    match result {
                        Ok(()) => {
                            tx.send(Ok(Response::Success))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
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
                CopyImage { image, timestamp, local} => {
                    let result = self.save_image(image, timestamp, local);
                    match result {
                        Ok(_) => {
                            tx.send(Ok(Response::Success))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
                        }
                    }
                }
                CopyText { text, timestamp, local } => {
                    let result = self.save_text(text, timestamp, local);
                    match result {
                        Ok(_) => {
                            tx.send(Ok(Response::Success))
                                .expect("failed to send response");
                        }
                        Err(e) => {
                            tx.send(Err(e.to_string()))
                                .expect("failed to send response");
                        }
                    }
                }
                Paste {
                    offset,
                    mut clipboard,
                } => {
                    let result = self.read_clipboard(offset);
                    let mut completed = true;
                    if result.is_ok() {
                        let r = result.unwrap();
                        use ClipboardEntry::*;
                        match r {
                            Image(i) => {
                                let i = i.into();
                                if (clipboard.inner.set_image(i)).is_err() {
                                    println!("failed to set image");
                                    completed = false;
                                }
                            }
                            Text(t) => {
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
                        tx.send(Ok(Response::Success))
                            .expect("failed to send response");
                    } else {
                        tx.send(Err("failed to paste".to_string()))
                            .expect("failed to send response");
                    }
                }
                History => match self.get_history() {
                    Ok(x) => {
                        tx.send(Ok(Response::History { names: x }))
                            .expect("failed to send response");
                    }
                    Err(e) => {
                        tx.send(Err(e.to_string()))
                            .expect("failed to send response");
                    }
                },
                Recent { length } => match self.get_recent(length) {
                    Ok(res) => {
                        tx.send(Ok(Response::Recent { values: res }))
                            .expect("failed to send response");
                    }
                    Err(e) => {
                        tx.send(Err(e.to_string()))
                            .expect("failed to send response");
                    }
                },
                InsertSelf { host_name } => match self.insert_self(host_name) {
                    Ok(()) => {
                        tx.send(Ok(Response::Success))
                            .expect("failed to send response");
                    }
                    Err(e) => {
                        tx.send(Err(e.to_string()))
                            .expect("failed to send response");
                    }
                },
                LoadClock => match self.load_clock() {
                    Ok(data) => {
                        tx.send(Ok(Response::Clock { data }))
                            .expect("failed to send response");
                    }
                    Err(e) => {
                        tx.send(Err(e.to_string()))
                            .expect("failed to send response");
                    }
                },
                SaveClock { clock } => match self.sync_clock(&clock) {
                    Ok(()) => {
                        tx.send(Ok(Response::Success))
                            .expect("failed to send response");
                    }
                    Err(e) => {
                        tx.send(Err(e.to_string()))
                            .expect("failed to send response");
                    }
                },
                _ => {}
            }
        }
    }
}

pub struct ClipboardWrapper {
    pub inner: arboard::Clipboard,
}

impl Debug for ClipboardWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clipboard")
    }
}

#[derive(Debug)]
pub enum DBCommand {
    Upload {
        file_name: String,
        file_path: String,
        timestamp: Ulid,
        local: bool
    },
    Download {
        download_path: String,
        file_name: String,
    },
    CopyImage {
        image: SerializableImage,
        timestamp: Ulid,
        local: bool,
    },
    CopyText {
        text: String,
        timestamp: Ulid,
        local: bool
    },
    Paste {
        offset: usize,
        clipboard: ClipboardWrapper,
    },
    ListFiles,
    History,
    Recent {
        length: u64,
    },
    InsertSelf {
        host_name: String,
    },
    LoadClock,
    SaveClock {
        clock: HashMap<String, u64>,
    },
}

#[derive(Debug)]
pub enum Response {
    Success,
    Files {
        names: Vec<String>,
    },
    History {
        names: Vec<String>,
    },
    Recent {
        values: Vec<(ClipboardEntry, String)>,
    },
    Clock {
        data: HashMap<String, u64>,
    },
}

#[derive(Debug)]
pub struct DBMessage {
    pub cmd: DBCommand,
    pub sender: Sender<Result<Response, String>>,
}
