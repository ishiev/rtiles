#[macro_use]
extern crate rocket;

use rocket::figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment, Profile,
};
use rocket::fs::NamedFile;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use rocket_cache_response::CacheResponse;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

mod config;
use config::{Config, SERVER_NAME, SERVER_VERSION};

mod model_access;
use model_access::{AccessConfig, AccessKey, AccessMode, ModelAccess, SessionId};

mod stat;
use stat::{Metrics, StatKey, Stat};

#[get("/models/<object>/<model>/<path..>")]
async fn tileset(
    object: &str,
    model: &str,
    path: PathBuf,
    session_id: SessionId<'_>,
    model_access: &State<ModelAccess>,
    config: &State<Arc<Config<'_>>>,
    stat: &State<Stat>
) -> Result<CacheResponse<NamedFile>, Status> {
    // check access mode for model
    let access_mode = model_access
        .check(AccessKey::new(Some(object), Some(model), session_id))
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
            match NamedFile::open(&file).await {
                Ok(res) => {
                    // prepare and insert stat
                    let bytes = match res.file().metadata().await {
                        Ok(meta) => meta.len(),
                        Err(_) => 0
                    };
                    let key = StatKey::new(Some(object), Some(model));
                    let metrics = Metrics { hits: 1, bytes };
                    stat.insert(key, metrics)
                        .await
                        .unwrap_or_else(|err| error!("error insert stat: {err}"));
                    // add cache header to response
                    Ok(CacheResponse::Private {
                        responder: res,
                        max_age: config.storage.max_age
                    })
                },
                Err(err) => {
                    warn!("error opening file {:?}: {}", &file, err);
                    Err(Status::NotFound)
                }
            }
        },
        AccessMode::Denied => Err(Status::Forbidden),
    }
}


#[get("/stat")]
async fn stat_all(
    session_id: SessionId<'_>,
    model_access: &State<ModelAccess>,
    stat: &State<Stat>
) -> Result<Json<Metrics>, Status> {
    // check access mode for object
    let access_mode = model_access
        .check(AccessKey::new(None, None, session_id))
        .await;

    match access_mode {
        AccessMode::Granted => Ok(Json(stat.get(&StatKey::new(None, None)).await)),
        AccessMode::Denied  => Err(Status::Forbidden)
    }
}

#[get("/stat/<object>")]
async fn stat_object(
    object: Option<&str>,
    session_id: SessionId<'_>,
    model_access: &State<ModelAccess>,
    stat: &State<Stat>
) -> Result<Json<Metrics>, Status> {
    // check access mode for object
    let access_mode = model_access
        .check(AccessKey::new(object, None, session_id))
        .await;

    match access_mode {
        AccessMode::Granted => Ok(Json(stat.get(&StatKey::new(object, None)).await)),
        AccessMode::Denied  => Err(Status::Forbidden)
    }
}

#[get("/stat/<object>/<model>")]
async fn stat_model(
    object: Option<&str>,
    model: Option<&str>,
    session_id: SessionId<'_>,
    model_access: &State<ModelAccess>,
    stat: &State<Stat>
) -> Result<Json<Metrics>, Status> {
    // check access mode for model
    let access_mode = model_access
        .check(AccessKey::new(object, model, session_id))
        .await;

    match access_mode {
        AccessMode::Granted => Ok(Json(stat.get(&StatKey::new(object, model)).await)),
        AccessMode::Denied  => Err(Status::Forbidden)
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

    println!("Starting 3D tiles rocket server, {}/{}", SERVER_NAME, SERVER_VERSION);

    rocket::custom(figment)
        .manage(model_access)
        .manage(Arc::clone(&config))
        .manage(Stat::new())
        .mount(
            config.base_path.to_owned(), 
            routes![tileset, stat_all, stat_object, stat_model]
        )
}
