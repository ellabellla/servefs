#[macro_use] extern crate rocket;
use std::{path::{PathBuf}, str::FromStr, net::IpAddr, fs};
use clap::{command, Parser};
use rocket::{State, http::{ContentType}, Config};
use servefs_lib::*;
use sqlx::{Row, sqlite::SqliteRow};
use tera::{Tera, Context};
use std::{str};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
   /// Location of database
   #[arg(short, long)]
   db: Option<String>,

   /// Location of templates directory
   #[arg(short, long)]
   templates: Option<String>,

   /// Location of directory template inside templates directory
   #[arg(long)]
   dir_template: Option<String>,

   // Port
   #[arg(short, long)]
   port: Option<u16>,

   // IP
   #[arg(short, long)]
   ip: Option<String>,
}

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

async fn render_dir(parent: &Directory, files: Vec<SqliteRow>, dirs: Vec<SqliteRow>, tera: &State<Tera>, dir_template: &State<String>) -> Option<(ContentType, Vec<u8>)> {
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
    let html = tera.render(dir_template, &context).ok()?;

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
async fn get_fs(path: PathBuf, fs_conn: &State<FSConnection>, tera: &State<Tera>, dir_template: &State<String>) -> Option<(ContentType, Vec<u8>)> {
    match fs_conn.resolve_path(path).await {
        Ok(fs_type) => match fs_type {
            FSType::File(file) => match file.read(&fs_conn).await {
                Ok((data, ftype)) => render_file(&get_ext(&file.name), data, ftype).await,
                Err(_) => None,
            },
            FSType::Directory(dir) => match dir.contents(fs_conn).await {
                Ok((files, dirs)) => render_dir(&dir, files, dirs, tera, dir_template).await,
                Err(_) => None,
            },
        },
        _=> None,
    }
}

#[launch]
async fn servefs() -> _ {
    let default_config_dir = "servefs/";
    let default_db_prefix = "sqlite://";
    let default_template_file = "directory.html";
    let default_directory_template = r#"<h1>{{parent}}</h1>
{% for dir in dirs %}
    <a href="{{dir}}">dir</a></br>
{% endfor %}
{% for file in files %}
    <a href="{{parent}}{{file}}">{{file}}</a></br>
{% endfor %}"#;

    let mut config = dirs::config_dir().expect("Could not find config path.");
    config.push(default_config_dir);
    fs::create_dir_all(&config).expect("Couldn't find a default config location");

    let args = Args::parse();
    
    let db_loc = match args.db {
        Some(db_loc) => db_loc,
        None => {
            let mut db_loc = config.clone();
            db_loc.push("fs.db");
            let db_loc = db_loc.to_str().expect("Couldn't find a default db location").to_string();
            format!("{}{}", default_db_prefix, db_loc)
        },
    };

    let mut template_path = config.clone();
    template_path.push("templates/");
    let template_loc = match args.templates {
        Some(template_loc) => {
            template_path = PathBuf::from_str(&template_loc).expect("Couldn't find template location");
            let mut template_loc = template_path.clone();
            template_loc.push("**/*");
            template_loc.to_str().expect("Couldn't find a default template location").to_string()
        },
        None => {
            let mut template_path = template_path.clone();
            fs::create_dir_all(&template_path).expect("Couldn't find a default template location");
            template_path.push("**/*");
            template_path.to_str().expect("Couldn't find a default template location").to_string()
        }
    };

    let dir_template_loc = match args.dir_template {
        Some(dir_template_loc) => {
            if !PathBuf::from_str(&dir_template_loc).expect("Couldn't find directory template").is_relative() {
                println!("Directory template location must be relative");
                ::std::process::exit(1);
            }
            template_path.push(&dir_template_loc);
            if !template_path.is_file() {
                println!("Couldn't find directory template");
                ::std::process::exit(1);
            }
            dir_template_loc
        },
        None => {
            template_path.push(&default_template_file);
            if !template_path.is_file() {
                println!("{:?}", template_path);
                fs::write(template_path, default_directory_template).expect("Couldn't create default directory template");
            }
            default_template_file.to_string()
        }
    };

    let mut rocket_config = Config::default();
    if let Some(port) = args.port {
        rocket_config.port = port;
    }
    if let Some(ip) = args.ip {
        rocket_config.address = match IpAddr::from_str(&ip) {
            Ok(ip) => ip,
            Err(e) => {
                println!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        }
    }

    let tera = match Tera::new(&template_loc) {
        Ok(t) => t,
        Err(e) => {
            println!("Parsing error(s): {}", e);
            ::std::process::exit(1);
        }
    };

    let fs_conn = FSConnection::new(&db_loc, "servefs_", true).await.unwrap();
    rocket::build()
        .configure(rocket_config)
        .manage(fs_conn)
        .manage(tera)
        .manage(dir_template_loc)
        .mount("/", routes![get_fs])
}
