#[macro_use] extern crate rocket;
use std::path::PathBuf;

#[get("/files/<path..>")]
fn get_fs(path: PathBuf) -> String {
    "".to_string()
}


#[launch]
fn servefs() -> _ {
    rocket::build()
        .mount("/", routes![get_fs])
}
