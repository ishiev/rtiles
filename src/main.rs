#[macro_use]
extern crate rocket;

use rocket::figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment, Profile,
};
use rocket::fs::NamedFile;
use rocket::http::{uri::Origin, CookieJar};
use rocket::serde::{Deserialize, Serialize};
use rocket::State;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use log::{info, error};

mod model_access;
use model_access::{AccessConfig, AccessKey, AccessMode, ModelAccess};

/// Configuration params for rtiles
#[derive(Debug, Deserialize, Serialize)]
struct Config<'a> {
    base_path: Origin<'a>,
    storage: ConfigStorage,
    access: AccessConfig,
}

impl Default for Config<'_> {
    fn default() -> Self {
        Config {
            base_path: Origin::path_only("/3d-models/model"),
            storage: ConfigStorage::default(),
            access: AccessConfig::default(),
        }
    }
}

/// Storage and cache params
#[derive(Debug, Deserialize, Serialize)]
struct ConfigStorage {
    root: PathBuf,
    cache_size: u32,
}

impl Default for ConfigStorage {
    fn default() -> Self {
        ConfigStorage {
            root: PathBuf::from("data"),
            cache_size: 1024 * 1024 * 512, // 512MB
        }
    }
}

#[get("/<object>/<model>/<path..>")]
async fn tileset(
    object: &str,
    model: &str,
    path: PathBuf,
    cookies: &CookieJar<'_>,
    model_access: &State<ModelAccess>,
    config: &State<Arc<Config<'_>>>,
) -> Option<NamedFile> {
 
    // get session id cookie from request
    let session_id = cookies
        .get(&model_access.config().cookie_name)
        .map(|x| x.value());
    
    // check access mode for model
    let acess_mode = model_access
        .check(AccessKey::new(object, model, session_id))
        .await;
        
    match acess_mode {
        AccessMode::Granted => {
            NamedFile::open(Path::new(&config.storage.root).join(path))
                       .await
                       .ok()
        },
        AccessMode::Denied => None,
    }
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
    let config = Arc::<Config>::new(figment.extract().unwrap_or_else(|err| {
        eprintln!("Problem parsing config: {err}");
        process::exit(1)
    }));

    // create model access cached resolver
    let model_access = ModelAccess::new(&config.access).unwrap_or_else(|err| {
        eprintln!("Problem create model access client: {err}");
        process::exit(1)
    });

    rocket::custom(figment)
        .manage(model_access)
        .manage(Arc::clone(&config))
        .mount(config.base_path.to_owned(), routes![tileset])
}
