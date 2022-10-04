#[macro_use] extern crate rocket;
use std::{path::{PathBuf, Path}, str::FromStr};
use rocket::{State, http::{ContentType}};
use servefs_lib::*;
use sqlx::{Row, sqlite::SqliteRow};
use std::str;

async fn exec(command: &str) -> Option<(ContentType, String)>{
    tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .output()
        .await
        .ok()
        .and_then(|out| 
            str::from_utf8(&out.stdout)
            .ok()
            .map(|out| out.to_string())
        )
        .map(|str| (ContentType::Text, str))
}

async fn render_file(data: String, ftype: String) -> Option<(ContentType, String)> {
    let ftype = match FileType::from_str(&ftype) {
        Ok(ftype) => ftype,
        Err(_) => return None,
    };

    match ftype {
        FileType::File => {
            let path = PathBuf::from_str(&data).ok()?;
            tokio::fs::read_to_string(path).await.ok()
                .map(|str| (ContentType::Text, str))
        },
        FileType::Text => Some((ContentType::Text, data)),
        FileType::Exec => tokio::time::timeout(
            tokio::time::Duration::from_secs(1), 
            exec(&data)
        ).await.ok().and_then(|o|o),
    }
}

async fn render_dir(parent: &Directory, files: Vec<SqliteRow>, dirs: Vec<SqliteRow>) -> Option<(ContentType, String)> {
    let dirs = dirs
        .iter()
        .map(|row| row.get::<String, &str>("directory"))
        .map(|name| format!("<a href=\"/files{}\">{}</a>", name, name));
    let files = files
        .iter()
        .map(|row| row.get::<String, &str>("name"))
        .map(|name| format!("<a href=\"/files{}{}\">{}</a>", parent.path, name, name));

    let contents = dirs.chain(files);

    let output = format!("<doctype html><html><body>{}</body></html>", contents.collect::<Vec<String>>().join("</br>"));

    Some((ContentType::HTML, output))
}

#[get("/files/<path..>")]
async fn get_fs(path: PathBuf, fs_conn: &State<FSConnection>) -> Option<(ContentType, String)> {
    match fs_conn.resolve_path(path).await {
        Ok(fs_type) => match fs_type {
            FSType::File(file) => match file.read(&fs_conn).await {
                Ok((data, ftype)) => render_file(data, ftype).await,
                Err(e) => None,
            },
            FSType::Directory(dir) => match dir.contents(fs_conn).await {
                Ok((files, dirs)) => render_dir(&dir, files, dirs).await,
                Err(_) => None,
            },
        },
        _=> None,
    }
}


#[launch]
async fn servefs() -> _ {
    let fs_conn = FSConnection::new("sqlite://fs.db", "servefs_", true).await.unwrap();
    rocket::build()
        .manage(fs_conn)
        .mount("/", routes![get_fs])
}
