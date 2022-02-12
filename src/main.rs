#[macro_use] extern crate rocket;

use std::path::PathBuf;

#[get("/<object>/<model>/<path..>")]
async fn tileset(object: &str, model: &str, path: PathBuf) -> String {
    format!("Hello! Object is: {object}, model is {model}, path is {path:?}!")
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .mount("/3d-models/model", routes![tileset])
}