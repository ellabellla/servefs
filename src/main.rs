use std::str::FromStr;

use sqlx::{SqlitePool, pool::PoolConnection, QueryBuilder, Sqlite, sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteRow}, Row};


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