/*#[macro_use] extern crate rocket;
use std::path::PathBuf;

#[get("/files/<a>")]
fn get_file(a: String) -> String {
    path.last().unwrap().to_string()
}


#[launch]
fn servefs() -> _ {
    rocket::build()
        .mount("/", routes![get_file])
}*/

use std::{str::FromStr, path::{PathBuf, self}};
use sqlx::{SqlitePool, sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteRow}, QueryBuilder, pool::PoolConnection, Sqlite, Row};
use path_absolutize::*;

#[derive(Debug)]
enum FSError {
    PathIsNotAFile(String),
    PathIsNotADir(String),
}

struct Directory {
    path: String,
}

impl Directory {
    fn path_to_str(path: PathBuf) -> Result<String, FSError> {
        match path.absolutize_virtually("/") {
            Ok(path) => Ok(format!("{}/", path.display().to_string())),
            Err(_) => Err(FSError::PathIsNotADir(path.display().to_string())),
        }        
    }

    pub fn new(path: PathBuf) -> Result<Directory, FSError>{
        Ok(Directory{path: Directory::path_to_str(path)?})
    }

    pub fn root() -> Directory {
        Directory { path: "/".to_string() }
    }

    pub async fn exists(&self, fs_sql: &FSSQL) -> Result<bool, sqlx::Error> {
        let mut conn = fs_sql.pool.acquire().await?;
       Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory=
            "#, fs_sql.dir_table))
            .push_bind(&self.path)
            .build()
            .fetch_optional(&mut conn)
            .await?
            .is_some())
    }

    pub async fn mk(&self, fs_sql: &FSSQL) -> Result<(), sqlx::Error> {
        let mut conn = fs_sql.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                INSERT INTO {}(directory) VALUES(
            "#, fs_sql.dir_table))
            .push_bind(&self.path)
            .push(");")
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn del(&self, fs_sql: &FSSQL) -> Result<(), sqlx::Error> {
        let mut conn = fs_sql.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                DELETE FROM {} where directory=
            "#, fs_sql.dir_table))
            .push_bind(&self.path)
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn rename(&mut self, name: &Directory, fs_sql: &FSSQL) -> Result<(), sqlx::Error> {
        let name = name.path.clone();
        let mut conn = fs_sql.pool.acquire().await?;
        QueryBuilder::new(format!("UPDATE {} SET directory=(",fs_sql.dir_table))
            .push_bind(&name)
            .push(format!(r#" || substr(directory, {})) WHERE directory LIKE "#, self.path.len()+1))
            .push_bind(format!("{}%", self.path))
            .build()
            .execute(&mut conn)
            .await?;
        
            self.path = name;
        Ok(())
    }

    pub async fn files(&self, fs_sql: &FSSQL) -> Result<Vec<SqliteRow>, sqlx::Error> {
        let mut conn = fs_sql.pool.acquire().await?;
        Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory=
            "#, fs_sql.file_table))
            .push_bind(&self.path)
            .build()
            .fetch_all(&mut conn)
            .await?)
    }

    pub async fn recurse(&self, fs_sql: &FSSQL) -> Result<(Vec<SqliteRow>, Vec<SqliteRow>), sqlx::Error> {
        let mut conn = fs_sql.pool.acquire().await?;
        Ok((QueryBuilder::new(format!(r#"
                SELECT name,type,directory FROM {} WHERE directory LIKE 
            "#, fs_sql.file_table))
            .push_bind(format!("{}%", &self.path))
            .build()
            .fetch_all(&mut conn)
            .await?,
            QueryBuilder::new(format!(r#"
                SELECT directory FROM {} WHERE directory LIKE 
            "#, fs_sql.dir_table))
            .push_bind(format!("{}%", &self.path))
            .build()
            .fetch_all(&mut conn)
            .await?,))
    }
}

struct FSSQL {
    pool: SqlitePool,
    file_table: String,
    dir_table: String,
    file_type_table: String,
}

impl FSSQL {
    async fn create_file_type_table(conn: &mut PoolConnection<Sqlite>, file_type_table: &str) -> Result<(), sqlx::Error>{
        QueryBuilder::new(format!(r#"
                CREATE TABLE {} (type TEXT PRIMARY KEY NOT NULL);
                INSERT INTO {} VALUES("file");
                INSERT INTO {} VALUES("text");
                INSERT INTO {} VALUES("exec");
            "#, file_type_table, file_type_table, file_type_table, file_type_table))
            .build()
            .execute(conn)
            .await?;
        Ok(())
    }

    async fn create_dir_table(conn: &mut PoolConnection<Sqlite>, dir_table: &str) -> Result<(), sqlx::Error>{
        QueryBuilder::new(format!(r#"
                CREATE TABLE {} (directory TEXT PRIMARY KEY NOT NULL CHECK(directory != "" AND (directory = "/" OR directory LIKE "/%/")));
                INSERT INTO {}(directory) VALUES("/");
            "#, dir_table, dir_table))
            .build()
            .execute(conn)
            .await?;
        Ok(())
    }

    async fn create_file_table(conn: &mut PoolConnection<Sqlite>, dir_table: &str, file_table: &str, file_type_table: &str) -> Result<(), sqlx::Error>{
        QueryBuilder::new(format!(r#"
                CREATE TABLE {} (id INTEGER PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL, directory TEXT NOT NULL, 
                    FOREIGN KEY(directory) REFERENCES {}(directory) ON DELETE CASCADE ON UPDATE CASCADE, 
                    FOREIGN KEY(type) REFERENCES {}(type) ON DELETE RESTRICT ON UPDATE RESTRICT, 
                    CONSTRAINT unq UNIQUE(name, directory));
            "#, file_table, dir_table, file_type_table))
            .build()
            .execute(conn)
            .await?;
        Ok(())
    }
    
    pub async fn new(filename: &str, table_prefix: &str, create_new: bool) -> Result<FSSQL, sqlx::Error> {
        let options = SqliteConnectOptions::from_str(filename)?
            .create_if_missing(create_new)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePool::connect_with(options).await?;
        let file_table = format!("{}{}", table_prefix, "files");
        let dir_table = format!("{}{}", table_prefix, "dirs");
        let file_type_table = format!("{}{}", table_prefix, "file_types");

        let mut conn = pool.acquire().await?;
        let found_tables: Vec<String> = QueryBuilder::new(r#"
                SELECT name FROM sqlite_master WHERE type="table" AND (name=
            "#)
            .push_bind(&file_table)
            .push(" OR name=")
            .push_bind(&dir_table)
            .push(" OR name=")
            .push_bind(&file_type_table)
            .push(")")
            .build()
            .fetch_all(&mut conn)
            .await?
            .iter()
            .map(|row| {row.get(0)})
            .collect();
        
        if !found_tables.contains(&file_type_table) {
            FSSQL::create_file_type_table(&mut conn, &file_type_table).await?;
        }
        if !found_tables.contains(&dir_table) {
            FSSQL::create_dir_table(&mut conn, &dir_table).await?;
        }
        if !found_tables.contains(&file_table) {
            FSSQL::create_file_table(&mut conn, &dir_table, &file_table, &file_type_table).await?;
        }

        Ok(FSSQL { pool, file_table, dir_table, file_type_table })
    }
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error>{
    let fsSQL = FSSQL::new("sqlite://fs.db", "servefs_", true).await?;
    Ok(())
}