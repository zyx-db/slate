use rusqlite::{params, Connection};
use std::{fs, io::Read};
use zstd::stream::encode_all;
use tokio::sync::mpsc::Receiver;

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
        ";

        connection.execute_batch(sql)?;
        
        Ok(Database { connection })
    }

    pub fn upload_file(&self, filename: &str, filepath: &str) -> Result<(), String> {
        println!("opening file from {} with name {}", filepath, filename);
        let mut file = fs::File::open(filepath).expect("cannot open file");
        let mut file_data = Vec::new();
        file.read_to_end(&mut file_data).expect("failed to read file");

        let compressed_data = encode_all(&file_data[..], 3).unwrap();
        self.connection.execute(
            "INSERT INTO files (file_name, content) VALUES (?1, ?2)",
            params![filename, compressed_data]
        ).unwrap();

        Ok(())
    }

    pub fn get_files(&self) -> Result<Vec<String>, rusqlite::Error> {
        let query = "
        SELECT f.file_name
        FROM files f;
        ";

        let mut statement = self.connection.prepare(query).expect("unable to prepare query");

        let res: Result<Vec<String>, rusqlite::Error> = statement.
            query_map([], |row| row.get::<usize, String>(0))?
            .collect();

        match res {
            Ok(res) => return Ok(res),
            Err(e) => return Err(e) 
        }
    }

    pub async fn listen(self, mut rx: Receiver<DBMessage>) {
        while let Some(_msg) = rx.recv().await {
        }
    }
}

pub struct DBMessage {}
