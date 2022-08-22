#[macro_use]
extern crate rocket;

use rocket::request::Request;
use rocket::response::Responder;
use rocket::serde::json::Json;
use rocket::State;
use rocket::{
    figment::{
        providers::{Env, Format, Serialized, Toml},
        Figment, Profile,
    },
    http::Status,
};
use rocket_cache_response::CacheResponse;
use std::{path::PathBuf, process};

mod model;
use model::Model;

mod meta;
use crate::meta::{Meta, MetaCache, MetaCacheConfig};

mod config;
use crate::config::{Config, SERVER_NAME, SERVER_VERSION};

mod access;
use crate::access::{AccessConfig, AccessKey, ModelAccess};

mod cache;
use crate::cache::{CachedNamedFile, FileCache, FileCacheConfig};

mod stat;
use stat::{Metrics, Stat, StatKey};

#[derive(Responder)]
enum Error {
    #[response(status = 404)]
    NotFound(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::NotFound(e.to_string())
    }
}

#[catch(default)]
fn default_catcher(status: Status, _: &Request) -> String {
    format!("{}", status)
}

#[get("/models/<_>/<_>/<path..>")]
async fn tileset(
    key: AccessKey,
    path: PathBuf,
    config: &State<Config<'_>>,
    cache: &State<FileCache>,
    metacache: &State<MetaCache>,
    stat: &State<Stat>,
) -> Result<CacheResponse<CachedNamedFile>, Error> {
    // build path to served file
    let mut file = PathBuf::from(&config.storage.root);
    file.push(&key.model.object.as_ref().unwrap());
    file.push(&key.model.name.as_ref().unwrap());
    file.push(&path);

    // get path metadata
    let mut meta = metacache.metadata(&file).await?;
    if meta.is_dir() {
        // if path is dir -- add default filename
        file.push("tileset.json");
        meta = metacache.metadata(&file).await?;
    }

    // serving file from disk or cache
    debug!("serving file: {:?}", &file);
    let res = CachedNamedFile::open_with_cache(&file, &meta, cache).await?;

    // prepare and insert stat
    let key = StatKey { model: key.model };
    let metrics = Metrics {
        hits: 1,
        cached: res.is_cached() as u64,
        bytes: res.meta().len(),
    };
    stat.insert(key, metrics)
        .await
        .unwrap_or_else(|err| error!("error insert stat: {err}"));

    // add cache header to response
    Ok(CacheResponse::Private {
        responder: res,
        max_age: config.storage.max_age,
    })
}

#[get("/stat/<_..>")]
async fn get_stat(key: AccessKey, stat: &State<Stat>) -> Json<Metrics> {
    let key = StatKey { model: key.model };
    Json(stat.get(&key).await)
}

#[get("/ping")]
async fn ping() -> &'static str {
    "pong"
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
    let config: Config = figment.extract().unwrap_or_else(|err| {
        eprintln!("Problem parsing config: {err}");
        process::exit(1)
    });

    // create model access cached resolver, exit if error
    let access = ModelAccess::new(&config.access).unwrap_or_else(|err| {
        eprintln!("Problem create model access client: {err}");
        process::exit(1)
    });

    // create file cache
    let cache = FileCache::new(FileCacheConfig {
        size: config.storage.cache_size,
    });

    // create metadata cache
    let metacache = MetaCache::new(MetaCacheConfig::default());

    // create stat server
    let stat = Stat::new();

    // set server base path from config
    let base_path = config.base_path.to_owned();

    println!(
        "Starting 3D tiles rocket server, {}/{}",
        SERVER_NAME, SERVER_VERSION
    );

    rocket::custom(figment)
        .manage(config)
        .manage(access)
        .manage(cache)
        .manage(metacache)
        .manage(stat)
        .mount(base_path, routes![tileset, get_stat, ping])
        .register("/", catchers![default_catcher])
}
