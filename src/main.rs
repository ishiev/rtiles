#[macro_use] extern crate rocket;

use std::path::PathBuf;
use std::process;
use rocket::serde::{Serialize, Deserialize};
use rocket::http::uri::Origin;
use rocket::figment::{Figment, Profile, providers::{Format, Toml, Serialized, Env}};


/// Configuration params for rtiles
#[derive(Debug, Deserialize, Serialize)]
struct Config {
    base_path: Origin<'static>,
    storage: ConfigStorage
}

impl Default for Config {
    fn default() -> Self {
        Config { 
            base_path: Origin::path_only("/3d-models/model"),
            storage: ConfigStorage::default()
        }
    }
}


/// Storage and cache params
#[derive(Debug, Deserialize, Serialize)]
struct ConfigStorage {
    root: PathBuf,
    cache_size: u32
}

impl Default for ConfigStorage {
    fn default() -> Self {
        ConfigStorage { 
            root: PathBuf::from("data"),
            cache_size: 1024*1024*512 // 512MB
        }
    }
}


#[get("/<object>/<model>/<path..>")]
async fn tileset(object: &str, model: &str, path: PathBuf) -> String {
    format!("Hello! Object is: {object}, model is {model}, path is {path:?}!")
}

#[launch]
fn rocket() -> _ {
    // set configutation sources
    let figment = Figment::from(rocket::Config::default())
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::file("rtiles.toml").nested())
        .merge(Env::prefixed("RTILES").global())
        .select(Profile::from_env_or("RTILES_PROFILE", "default"));

    // extract the config, exit if error
    let config: Config = figment.extract()
        .unwrap_or_else(|err| {
            eprintln!("Problem parsing config: {err}");
            process::exit(1)
        });

    rocket::custom(figment)
        .mount(config.base_path, routes![tileset])
}