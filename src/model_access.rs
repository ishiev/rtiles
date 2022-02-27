use moka::future::Cache;
use reqwest::{Client, Error, StatusCode};
use rocket::http::uri::Absolute;
use rocket::serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

/// Model auth configuration
/// TODO: write docs
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct AccessConfig {
    pub server: Absolute<'static>,
    pub cache_ttl: u64,     // cache entry Time To Live
    pub cache_tti: u64,     // cache entry Time To Idle (from last request)
    pub cookie_name: Cow<'static, str>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        AccessConfig {
            server: uri!("http://127.0.0.1:8888"),
            cache_ttl: 30*60,   // 30 minutes
            cache_tti: 5*60,    // 5 minutes    
            cookie_name: Cow::from("PHPSESSID"),
        }
    }
}

/// Model access mode
#[derive(Debug, Clone, PartialEq)]
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
            // Max TTL for items
            .time_to_live(Duration::from_secs(config.cache_ttl))
            // Max TTI for items - 5 min
            .time_to_idle(Duration::from_secs(config.cache_tti))
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

    // check access to model
    pub async fn check(&self, key: AccessKey) -> AccessMode {
        let key  = Arc::new(key);
        let key2 = Arc::clone(&key);
        let mode = self.cache
            .get_or_insert_with(key, async { self.check_remote(&key2).await })
            .await;
        debug!("access {:?} for {:?}", mode, &key2);
        mode
    }

    async fn check_remote(&self, key: &AccessKey) -> AccessMode {
        // url for request
        let url = format!("{}/{}/{}",
            self.config.server, 
            key.object, 
            key.model
        );
        
        // prepare request to remote server
        debug!("request to remote server: {}", &url);
        let mut rq = self.client.get(&url);
            
        // add session id cookie if exists
        if let Some(id) = &key.session_id {
            let cookie = format!(
                "{}={}", 
                self.config.cookie_name, 
                id
            );
            debug!("set cookie: {}", &cookie);
            rq = rq.header("Cookie", &cookie);
        } 
       
        // send request to remote server and interpret response
        match rq.send().await {
            Ok(res) if res.status() == StatusCode::OK => AccessMode::Granted,
            Ok(_)  => AccessMode::Denied,
            Err(err) => {
                error!("failed to get response from remote server: {}", &err);
                AccessMode::Denied
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn get_model_access(server: &'static str) -> ModelAccess {
        let mut config = AccessConfig::default();
        config.server = Absolute::parse(server).unwrap();
        ModelAccess::new(&config).unwrap()
    }

    fn get_access_key() -> AccessKey {
        AccessKey::new("tver", "panorama", Some("secret_key"))
    }

    #[test]
    fn default_config() {
        let cfg = AccessConfig::default();
        assert_eq!(cfg, AccessConfig {
            server: uri!("http://127.0.0.1:8888"),
            cache_ttl: 30*60, 
            cache_tti: 5*60,   
            cookie_name: Cow::from("PHPSESSID"),
        })
    }

    #[test]
    fn create_key() {
        assert_eq!(get_access_key(), AccessKey {
            object: String::from("tver"),
            model: String::from("panorama"),
            session_id: Some(String::from("secret_key"))
        })
    }

    #[rocket::async_test]
    async fn access_check_timeout() {
        let key = get_access_key();
        // set auth server to non routable address from TEST-NET-1
        // this cause to timeout 5c
        let model_access = get_model_access("http://192.0.2.0");  
        assert_eq!(model_access.check(key).await, AccessMode::Denied)
    }

    #[rocket::async_test]
    async fn access_check_granted() {
        let key = get_access_key();
        // set auth server to test server, always returns 200 OK
        let model_access = get_model_access("https://httpbin.org/anything");
        assert_eq!(model_access.check(key).await, AccessMode::Granted)
    }

    #[rocket::async_test]
    async fn access_check_denied() {
        let key = get_access_key();
        // set auth server to test server, returns 404 NOT FOUND
        let model_access = get_model_access("https://httpbin.org/status/404");
        assert_eq!(model_access.check(key).await, AccessMode::Denied)
    }
}
