use rocket::http::uri::Origin;
use rocket::serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::AccessConfig;

pub const SERVER_NAME: &str = env!("CARGO_PKG_NAME");
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Configuration params for rtiles
#[derive(Debug, Deserialize, Serialize)]
pub struct Config<'a> {
    pub ident: String,
    pub cli_colors: bool,
    pub base_path: Origin<'a>,
    pub storage: ConfigStorage,
    pub access: AccessConfig,
}

impl Default for Config<'_> {
    fn default() -> Self {
        Config {
            ident: format!("{}/{}", SERVER_NAME, SERVER_VERSION),
            cli_colors: false,
            base_path: Origin::path_only("/3d"),
            storage: ConfigStorage::default(),
            access: AccessConfig::default(),
        }
    }
}

/// Storage and client cache params
#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigStorage {
    pub root: PathBuf,
    pub max_age: u32,
    pub cache_size: u64
}

impl Default for ConfigStorage {
    fn default() -> Self {
        ConfigStorage {
            root: PathBuf::from("data"),
            max_age: 30 * 60,  // 30 minutes
            cache_size: 500,   // 500 MB  
        }
    }
}
