use moka::future::Cache;
use reqwest::{Client, Error, StatusCode};
use rocket::http::uri::Absolute;
use rocket::serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use log::{info, error};

/// Model auth configuration
/// TODO: write docs
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AccessConfig {
    pub server: Absolute<'static>,
    pub cache_ttl: u64,
    pub cookie_name: Cow<'static, str>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        AccessConfig {
            server: uri!("http://127.0.0.1:8888"),
            cache_ttl: 30 * 60, // 30 minutes
            cookie_name: Cow::from("PHPSESSID"),
        }
    }
}

/// Model access mode
#[derive(Debug, Clone)]
pub enum AccessMode {
    Granted,
    Denied,
}

/// Model Access key
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct AccessKey {
    object: String,
    model: String,
    session_id: Option<String>,
}

impl AccessKey {
    pub fn new(object: &str, model: &str, session_id: Option<&str>) -> Self {
        AccessKey{
            object: object.to_owned(),
            model: model.to_owned(),
            session_id: session_id.map(str::to_owned)
        }
    }
}

/// Model Access resolver
pub struct ModelAccess {
    cache: Cache<Arc<AccessKey>, AccessMode>,
    client: Client,
    config: AccessConfig,
}

impl ModelAccess {
    pub fn new(config: &AccessConfig) -> Result<Self, Error> {
        let cache = Cache::builder()
            // Max 100,000 entries
            .max_capacity(100_000)
            // Max TTL fof items
            .time_to_idle(Duration::from_secs(config.cache_ttl))
            .build();

        let client = Client::builder()
            // Timeout 5s for request to remote server
            .timeout(Duration::from_secs(5))
            .build()?;

        Ok(ModelAccess {
            cache,
            client,
            config: config.clone(),
        })
    }

    // get back config reference
    pub fn config(&self) -> &AccessConfig {
        &self.config
    }

    // check access to model
    pub async fn check(&self, key: AccessKey) -> AccessMode {
        let key = Arc::new(key);
        let key2 = key.clone();
        self.cache
            .get_or_insert_with(key, async { self.check_remote(&key2).await })
            .await
    }

    async fn check_remote(&self, key: &AccessKey) -> AccessMode {
        // url for request
        let url = format!("{}/{}/{}",
            self.config.server, 
            key.object, 
            key.model
        );
        
        info!("Send request to remote server: {}", &url);

        // prepare request to remote server
        let mut rq = self.client.get(&url);
            
        // add session id cookie if exists
        if let Some(id) = &key.session_id {
            rq = rq.header(
                "Cookie",
                format!(
                    "{}={}", 
                    self.config.cookie_name, 
                    id
                )
            )
        } 
       
        // send request to remote server and interpret response
        match rq.send().await {
            Ok(res) if res.status() == StatusCode::OK => AccessMode::Granted,
            Ok(_)  => AccessMode::Denied,
            Err(_) => AccessMode::Denied
        }
    }
}
