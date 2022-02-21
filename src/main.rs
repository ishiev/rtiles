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
use rocket_cache_response::CacheResponse;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

mod model_access;
use model_access::{AccessConfig, AccessKey, AccessMode, ModelAccess};

const SERVER_NAME: &str = env!("CARGO_PKG_NAME");
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Configuration params for rtiles
#[derive(Debug, Deserialize, Serialize)]
struct Config<'a> {
    ident: String,
    cli_colors: bool,
    base_path: Origin<'a>,
    storage: ConfigStorage,
    access: AccessConfig,
}

impl Default for Config<'_> {
    fn default() -> Self {
        Config {
            ident: format!("{}/{}", SERVER_NAME, SERVER_VERSION),
            cli_colors: false,
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
    max_age: u32,
}

impl Default for ConfigStorage {
    fn default() -> Self {
        ConfigStorage {
            root: PathBuf::from("data"),
            max_age: 30*60, // 30 minutes
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
) -> Option<CacheResponse<NamedFile>> {
 
    // get session id cookie from request
    let session_id = cookies
        .get(&config.access.cookie_name)
        .map(|x| x.value());
    
    // check access mode for model
    let access_mode = model_access
        .check(AccessKey::new(object, model, session_id))
        .await;
        
    match access_mode {
        AccessMode::Granted => {
            // build path to served file
            let mut file = PathBuf::from(&config.storage.root);
            file.push(&object);
            file.push(&model);
            file.push(&path);
            if file.is_dir() {
                // if file path is directory -- assume tileset.json file
                file.push("tileset.json");
            }
            // serving file with cache-control header set
            debug!("serving file: {:?}", &file);
            NamedFile::open(file)
                .await
                .ok()
                .map(|x| CacheResponse::Private {
                    responder: x,
                    max_age: config.storage.max_age
                })          
        },
        AccessMode::Denied => None,
    }
}

#[catch(404)]
fn not_found() -> &'static str {
    "NOT FOUND"
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

    println!("Starting 3D tiles rocket server, {}/{}", SERVER_NAME, SERVER_VERSION);

    rocket::custom(figment)
        .manage(model_access)
        .manage(Arc::clone(&config))
        .mount(config.base_path.to_owned(), routes![tileset])
        .register("/", catchers![not_found])
}
