#[macro_use] extern crate rocket;
use std::{path::{PathBuf}, str::FromStr};
use rocket::{State, http::{ContentType}};
use servefs_lib::*;
use sqlx::{Row, sqlite::SqliteRow};
use tera::{Tera, Context};
use std::str;

async fn exec(ext: &str, command: &str) -> Option<(ContentType, Vec<u8>)>{
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
        .map(|str| (ContentType::from_extension(ext).unwrap_or(ContentType::Text), str.as_bytes().to_vec()))
}

async fn render_file(ext: &str, data: String, ftype: String) -> Option<(ContentType, Vec<u8>)> {
    let ftype = match FileType::from_str(&ftype) {
        Ok(ftype) => ftype,
        Err(_) => return None,
    };

    match ftype {
        FileType::File => {
            let path = PathBuf::from_str(&data).ok()?;
            tokio::fs::read(path).await.ok()
                .map(|str| (ContentType::from_extension(ext).unwrap_or(ContentType::Text), str))
        },
        FileType::Text => Some((ContentType::from_extension(ext).unwrap_or(ContentType::Text), data.as_bytes().to_vec())),
        FileType::Exec => tokio::time::timeout(
            tokio::time::Duration::from_secs(1), 
            exec(ext, &data)
        ).await.ok().and_then(|o|o),
    }
}

async fn render_dir(parent: &Directory, files: Vec<SqliteRow>, dirs: Vec<SqliteRow>, tera: &State<Tera>) -> Option<(ContentType, Vec<u8>)> {
    let mut dirs = dirs
        .iter()
        .map(|row| row.get::<String, &str>("directory"))
        .map(|name| name)
        .collect::<Vec<String>>();
    let mut files = files
        .iter()
        .map(|row| row.get::<String, &str>("name"))
        .map(|name| name)
        .collect::<Vec<String>>();

    dirs.sort();
    files.sort();

    let mut context = Context::new();
    context.insert("dirs", &dirs);
    context.insert("files", &files);
    context.insert("parent", &parent.path);
    let html = tera.render("directory.html", &context).ok()?;

    Some((ContentType::HTML, html.as_bytes().to_vec()))
}

fn get_ext(name: &str) -> String {
    let path = match PathBuf::from_str(name){
        Ok(path) => path,
        Err(_) => return "".to_string(),
    };
    path.extension().map(|ostr| 
        ostr.to_str().map(|str| str.to_string()).unwrap_or("".to_string())
    ).unwrap_or("".to_string())
}

#[get("/<path..>")]
async fn get_fs(path: PathBuf, fs_conn: &State<FSConnection>, tera: &State<Tera>) -> Option<(ContentType, Vec<u8>)> {
    match fs_conn.resolve_path(path).await {
        Ok(fs_type) => match fs_type {
            FSType::File(file) => match file.read(&fs_conn).await {
                Ok((data, ftype)) => render_file(&get_ext(&file.name), data, ftype).await,
                Err(_) => None,
            },
            FSType::Directory(dir) => match dir.contents(fs_conn).await {
                Ok((files, dirs)) => render_dir(&dir, files, dirs, tera).await,
                Err(_) => None,
            },
        },
        _=> None,
    }
}


#[launch]
async fn servefs() -> _ {
    let tera = match Tera::new("templates/**/*") {
        Ok(t) => t,
        Err(e) => {
            println!("Parsing error(s): {}", e);
            ::std::process::exit(1);
        }
    };

    let fs_conn = FSConnection::new("sqlite://fs.db", "servefs_", true).await.unwrap();
    rocket::build()
        .manage(fs_conn)
        .manage(tera)
        .mount("/", routes![get_fs])
}
