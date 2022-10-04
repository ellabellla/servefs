

use std::{str::FromStr, path::{PathBuf}};
use sqlx::{SqlitePool, sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteRow}, QueryBuilder, pool::PoolConnection, Sqlite, Row};
use path_absolutize::*;

pub enum FSType {
    File(File),
    Directory(Directory),
}

#[derive(Debug)]
pub enum FSError {
    PathIsNotAFile(String),
    PathIsNotADir(String),
    DoesNotExist(String),
    InvalidType(String),
    SqlX(sqlx::Error),
}

pub enum FileType {
    File,
    Text,
    Exec
}

impl ToString for FileType {
    fn to_string(&self) -> String {
        match self {
            FileType::File => String::from("file"),
            FileType::Text => String::from("text"),
            FileType::Exec => String::from("exec"),
        }
    }
}

impl FromStr for FileType {
    type Err = FSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "file" => Ok(FileType::File),
            "text" => Ok(FileType::Text),
            "exec" => Ok(FileType::Exec),
            _ => Err(FSError::InvalidType(s.to_string()))
        }
    }
}

pub struct File {
    name: String,
    directory: Directory,
}

impl File {
    fn path_to_str(path: &PathBuf) ->  Result<String, FSError> {
        match path.file_name() {
            Some(name) => Ok(name.to_string_lossy().to_string()),
            None => return Err(FSError::PathIsNotAFile(path.display().to_string())),
        }
    }

    pub fn new(path: PathBuf) -> Result<File, FSError>{
        let name = File::path_to_str(&path)?;

        let path = match path.absolutize_virtually("/") {
            Ok(path) => path,
            Err(_) => Err(FSError::PathIsNotAFile(path.display().to_string()))?,
        };

        let directory = match path.parent() {
            Some(directory) => Directory::new(directory.to_path_buf())?,
            None => Directory::root(),
        };

        Ok(File{name, directory})
    }

    pub async fn exists(&self, fs_conn: &FSConnection) -> Result<bool, sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory=
            "#, fs_conn.file_table))
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .fetch_optional(&mut conn)
            .await?
            .is_some())
    }

    pub async fn mk(&self, data:&str, ftype: &FileType, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                INSERT INTO {}(name,type,data,directory) VALUES(
            "#, fs_conn.file_table))
            .push_bind(&self.name)
            .push(",")
            .push_bind(&ftype.to_string())
            .push(",")
            .push_bind(&data)
            .push(",")
            .push_bind(&self.directory.path)
            .push(");")
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn del(&self, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                DELETE FROM {} where directory=
            "#, fs_conn.file_table))
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn rename(&mut self, name: &str, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let name = name.to_string();
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                UPDATE {} SET name=
            "#,fs_conn.file_table))
            .push_bind(&name)
            .push("WHERE directory=")
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .execute(&mut conn)
            .await?;
        
            self.name = name;
        Ok(())
    }

    pub async fn mv(&mut self, directory: Directory, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                UPDATE {} SET directory=
            "#,fs_conn.file_table))
            .push_bind(&directory.path)
            .push("WHERE directory=")
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .execute(&mut conn)
            .await?;
        
            self.directory = directory;
        Ok(())
    }

    pub async fn read(&self, fs_conn: &FSConnection) -> Result<(String, String), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        let row = QueryBuilder::new(format!(r#"
                SELECT data,type FROM {} WHERE directory=
            "#, fs_conn.file_table))
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .fetch_one(&mut conn)
            .await?;

        Ok((row.try_get("data")?, row.try_get("type")?))
    }

    pub async fn write(&mut self, data: &str, ftype: FileType, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                UPDATE {} SET data=
            "#,fs_conn.file_table))
            .push_bind(&data)
            .push(", type=")
            .push_bind(&ftype.to_string())
            .push("WHERE directory=")
            .push_bind(&self.directory.path)
            .push("AND name=")
            .push_bind(&self.name)
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }
}

pub struct Directory {
    pub path: String,
}

impl Directory {
    fn path_to_str(path: PathBuf) -> Result<String, FSError> {
        match path.absolutize_virtually("/") {
            Ok(path) => {
                let path = path.display().to_string();
                if path.ends_with('/') {
                    Ok(path)
                } else {
                    Ok(format!("{}/", path))
                }
            },
            Err(_) => Err(FSError::PathIsNotADir(path.display().to_string())),
        }        
    }

    pub fn new(path: PathBuf) -> Result<Directory, FSError>{
        Ok(Directory{path: Directory::path_to_str(path)?})
    }

    pub fn root() -> Directory {
        Directory { path: "/".to_string() }
    }

    pub async fn exists(&self, fs_conn: &FSConnection) -> Result<bool, sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
       Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory=
            "#, fs_conn.dir_table))
            .push_bind(&self.path)
            .build()
            .fetch_optional(&mut conn)
            .await?
            .is_some())
    }

    pub async fn mk(&self, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                INSERT INTO {}(directory) VALUES(
            "#, fs_conn.dir_table))
            .push_bind(&self.path)
            .push(");")
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    // Make recursion
    pub async fn del(&self, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!(r#"
                DELETE FROM {} where directory=
            "#, fs_conn.dir_table))
            .push_bind(&self.path)
            .build()
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    pub async fn mv(&mut self, path: &Directory, fs_conn: &FSConnection) -> Result<(), sqlx::Error> {
        let path = path.path.clone();
        let mut conn = fs_conn.pool.acquire().await?;
        QueryBuilder::new(format!("UPDATE {} SET directory=(",fs_conn.dir_table))
            .push_bind(&path)
            .push(format!(r#" || substr(directory, {})) WHERE directory LIKE "#, self.path.len()+1))
            .push_bind(format!("{}%", self.path))
            .build()
            .execute(&mut conn)
            .await?;
        
            self.path = path;
        Ok(())
    }

    pub fn rename(&self, name: &str) -> Result<PathBuf, FSError> {
        let mut path = match PathBuf::from_str(&self.path) {
            Ok(path) => path,
            Err(_) => Err(FSError::PathIsNotADir(self.path.clone()))?,
        };
        let new_name = match PathBuf::from_str(&name) {
            Ok(path) => path,
            Err(_) => Err(FSError::PathIsNotADir(name.to_string()))?,
        };
        path.pop();
        path.push(new_name);

        Ok(path)
    }

    pub async fn files(&self, fs_conn: &FSConnection) -> Result<Vec<SqliteRow>, sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory=
            "#, fs_conn.file_table))
            .push_bind(&self.path)
            .build()
            .fetch_all(&mut conn)
            .await?)
    }

    pub async fn dirs(&self, fs_conn: &FSConnection) -> Result<Vec<SqliteRow>, sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        Ok(QueryBuilder::new(format!(r#"
                SELECT * FROM {} WHERE directory LIKE
            "#, fs_conn.dir_table))
            .push_bind(&format!("{}%/", self.path))
            .build()
            .fetch_all(&mut conn)
            .await?)
    }

    pub async fn contents(&self, fs_conn: &FSConnection) -> Result<(Vec<SqliteRow>, Vec<SqliteRow>), sqlx::Error>  {
        Ok((self.files(fs_conn).await?, self.dirs(fs_conn).await?))
    }

    pub async fn recurse(&self, fs_conn: &FSConnection) -> Result<(Vec<SqliteRow>, Vec<SqliteRow>), sqlx::Error> {
        let mut conn = fs_conn.pool.acquire().await?;
        Ok((QueryBuilder::new(format!(r#"
                SELECT name,type,directory FROM {} WHERE directory LIKE 
            "#, fs_conn.file_table))
            .push_bind(format!("{}%", &self.path))
            .build()
            .fetch_all(&mut conn)
            .await?,
            QueryBuilder::new(format!(r#"
                SELECT directory FROM {} WHERE directory LIKE 
            "#, fs_conn.dir_table))
            .push_bind(format!("{}%", &self.path))
            .push("AND directory!=")
            .push_bind(&self.path)
            .build()
            .fetch_all(&mut conn)
            .await?,))
    }
}

pub struct FSConnection {
    pool: SqlitePool,
    pub file_table: String,
    pub dir_table: String,
    pub file_type_table: String,
}

impl FSConnection {
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
    
    async fn find_tables(conn: &mut PoolConnection<Sqlite>, dir_table: &str, file_table: &str, file_type_table: &str) -> Result<Vec<String>, sqlx::Error> {
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
            .fetch_all(conn)
            .await?
            .iter()
            .map(|row| {row.get(0)})
            .collect();
        Ok(found_tables)
    }

    fn create_table_names(table_prefix: &str) -> (String, String, String) {
        let file_table = format!("{}{}", table_prefix, "files");
        let dir_table = format!("{}{}", table_prefix, "dirs");
        let file_type_table = format!("{}{}", table_prefix, "file_types");

        (file_table, dir_table, file_type_table)
    }

    pub async fn new(filename: &str, table_prefix: &str, create_new: bool) -> Result<FSConnection, sqlx::Error> {
        let options = SqliteConnectOptions::from_str(filename)?
            .create_if_missing(create_new)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePool::connect_with(options).await?;
        let (file_table, dir_table, file_type_table) = FSConnection::create_table_names(table_prefix);

        let mut conn = pool.acquire().await?;
        let found_tables = FSConnection::find_tables(&mut conn, &dir_table, &file_table, &file_type_table).await?;
        
        if !found_tables.contains(&file_type_table) {
            FSConnection::create_file_type_table(&mut conn, &file_type_table).await?;
        }
        if !found_tables.contains(&dir_table) {
            FSConnection::create_dir_table(&mut conn, &dir_table).await?;
        }
        if !found_tables.contains(&file_table) {
            FSConnection::create_file_table(&mut conn, &dir_table, &file_table, &file_type_table).await?;
        }

        Ok(FSConnection { pool, file_table, dir_table, file_type_table })
    }

    pub async fn resolve_path(&self, path: PathBuf) -> Result<FSType, FSError> {
        match Directory::new(path.clone()) {
            Ok(dir) => {
                if match dir.exists(&self).await {
                    Ok(found) => found,
                    Err(e) => Err(FSError::SqlX(e))?,
                } {
                    return Ok(FSType::Directory(dir))
                }        
            },
            _ => (),
        }
        match File::new(path.clone()) {
            Ok(file) => {
                if match file.exists(&self).await {
                    Ok(found) => found,
                    Err(e) => Err(FSError::SqlX(e))?,
                } {
                    return Ok(FSType::File(file))
                }
            },
            _ => (),
        }

        Err(FSError::DoesNotExist(path.display().to_string()))
    }
}


#[cfg(test)]
mod tests {

    use std::{str::FromStr, path::PathBuf};

    use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode}, SqlitePool, Row};

    use crate::{FSConnection, File, FileType, Directory, FSType};

    async fn remove_test_db() {
        match tokio::fs::remove_file("./test.db").await {
            Ok(_) => (),
            Err(_) => (),
        }
        match tokio::fs::remove_file("./test.db-shm").await {
            Ok(_) => (),
            Err(_) => (),
        }
        match tokio::fs::remove_file("./test.db-wal").await {
            Ok(_) => (),
            Err(_) => (),
        }
    }

    #[tokio::test]
    async fn test_fs_connection() {
        remove_test_db().await;

        {
            let fs_conn = FSConnection::new("sqlite://test.db", "servefs_", true).await.unwrap();

            let file = File::new(PathBuf::from_str("/file").unwrap()).unwrap();
            file.mk("data", &FileType::Text, &fs_conn).await.unwrap();
            assert!(file.exists(&fs_conn).await.unwrap());

            let dir = Directory::new(PathBuf::from_str("/h/").unwrap()).unwrap();
            dir.mk(&fs_conn).await.unwrap();
            assert!(dir.exists(&fs_conn).await.unwrap());

            assert!(matches!(fs_conn.resolve_path(PathBuf::from_str("/file").unwrap()).await.unwrap(), FSType::File(_)));
            assert!(matches!(fs_conn.resolve_path(PathBuf::from_str("/h").unwrap()).await.unwrap(), FSType::Directory(_)));
        }

        let options = SqliteConnectOptions::from_str("sqlite://test.db").unwrap()
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePool::connect_with(options).await.unwrap();
        let (file_table, dir_table, file_type_table) = FSConnection::create_table_names("servefs_");

        let mut conn = pool.acquire().await.unwrap();
        let found_tables = FSConnection::find_tables(&mut conn, &dir_table, &file_table, &file_type_table).await.unwrap();

        assert!(found_tables.contains(&file_type_table));
        assert!(found_tables.contains(&dir_table));
        assert!(found_tables.contains(&file_table));


        remove_test_db().await;
    }

    #[tokio::test]
    async fn test_file() {
        remove_test_db().await;

        let fs_conn = FSConnection::new("sqlite://test.db", "servefs_", true).await.unwrap();
        let mut file = File::new(PathBuf::from_str("/file").unwrap()).unwrap();
        
        assert!(!file.exists(&fs_conn).await.unwrap());
        file.mk("data", &FileType::Text, &fs_conn).await.unwrap();
        assert!(file.exists(&fs_conn).await.unwrap());

        file.rename("file_2", &fs_conn).await.unwrap();
        assert!(file.exists(&fs_conn).await.unwrap());
        assert_eq!(file.name, "file_2");

        let new_dir = Directory::new(PathBuf::from_str("/home/").unwrap()).unwrap();
        new_dir.mk(&fs_conn).await.unwrap();



        file.mv(new_dir, &fs_conn).await.unwrap();
        assert!(file.exists(&fs_conn).await.unwrap());
        assert_eq!(file.directory.path, "/home/");

        let (data, ftype) = file.read(&fs_conn).await.unwrap();
        assert_eq!(data, "data");
        assert_eq!(ftype, FileType::Text.to_string());

        file.write("a program", FileType::Exec, &fs_conn).await.unwrap();
        let (data, ftype) = file.read(&fs_conn).await.unwrap();
        assert_eq!(data, "a program");
        assert_eq!(ftype, FileType::Exec.to_string());

        file.del(&fs_conn).await.unwrap();
        assert!(!file.exists(&fs_conn).await.unwrap());

        remove_test_db().await;
    }

    #[tokio::test]
    async fn test_directory() {
        remove_test_db().await;

        let fs_conn = FSConnection::new("sqlite://test.db", "servefs_", true).await.unwrap();
        let mut dir = Directory::new(PathBuf::from_str("/h/").unwrap()).unwrap();
        let sub_a = Directory::new(PathBuf::from_str("/h/a").unwrap()).unwrap();
        let sub_b = Directory::new(PathBuf::from_str("/h/b").unwrap()).unwrap();
        let file = File::new(PathBuf::from_str("/h/file").unwrap()).unwrap();

        assert!(!dir.exists(&fs_conn).await.unwrap());
        dir.mk(&fs_conn).await.unwrap();
        assert!(dir.exists(&fs_conn).await.unwrap());
        
        sub_a.mk(&fs_conn).await.unwrap();
        assert!(sub_a.exists(&fs_conn).await.unwrap());
        sub_b.mk(&fs_conn).await.unwrap();
        assert!(sub_b.exists(&fs_conn).await.unwrap());
        file.mk("data", &FileType::Text, &fs_conn).await.unwrap();
        assert!(file.exists(&fs_conn).await.unwrap());

        dir.mv(&Directory::new(dir.rename("home").unwrap()).unwrap(), &fs_conn).await.unwrap();
        
        assert!(dir.exists(&fs_conn).await.unwrap());
        assert_eq!(dir.path, "/home/");
        assert!(!sub_a.exists(&fs_conn).await.unwrap());
        assert!(!sub_b.exists(&fs_conn).await.unwrap());
        assert!(!file.exists(&fs_conn).await.unwrap());
        let sub_a = Directory::new(PathBuf::from_str("/home/a").unwrap()).unwrap();
        let sub_b = Directory::new(PathBuf::from_str("/home/b").unwrap()).unwrap();
        let file = File::new(PathBuf::from_str("/home/file").unwrap()).unwrap();
        assert!(sub_a.exists(&fs_conn).await.unwrap());
        assert!(sub_b.exists(&fs_conn).await.unwrap());
        assert!(file.exists(&fs_conn).await.unwrap());

        let file_a = File::new(PathBuf::from_str("/home/a/file_a").unwrap()).unwrap();
        file_a.mk("data", &FileType::Text, &fs_conn).await.unwrap();
        assert!(file_a.exists(&fs_conn).await.unwrap());

        let files = dir.files(&fs_conn).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].get::<String, &str>("name"), "file");

        let dirs: Vec<String>= dir.dirs(&fs_conn).await.unwrap().iter().map(|r| r.get("directory")).collect();
        assert_eq!(dirs.len(), 2);
        assert!(dirs.contains(&"/home/a/".to_string()));
        assert!(dirs.contains(&"/home/b/".to_string()));

        let all = dir.recurse(&fs_conn).await.unwrap();
        assert_eq!(all.0.len(), 2);
        assert_eq!(all.1.len(), 2);

        let files: Vec<String>= all.0.iter().map(|r| r.get("name")).collect();
        assert!(files.contains(&"file".to_string()));
        assert!(files.contains(&"file_a".to_string()));

        let dirs: Vec<String>= all.1.iter().map(|r| r.get("directory")).collect();
        assert!(dirs.contains(&"/home/a/".to_string()));
        assert!(dirs.contains(&"/home/b/".to_string()));

        remove_test_db().await;
    }
}