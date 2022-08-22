use moka::future::Cache;
use std::{
    fs::Metadata,
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    len: u64,
    modified: Option<SystemTime>,
    is_dir: bool,
}

impl From<Metadata> for Meta {
    fn from(metadata: Metadata) -> Self {
        Meta {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            is_dir: metadata.is_dir(),
        }
    }
}

impl Meta {
    pub async fn from_path(path: &Path) -> io::Result<Meta> {
        Ok(Meta::from(tokio::fs::metadata(path).await?))
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
}


/// Metadata cache configuration
#[derive(Debug, Clone, PartialEq)]
pub struct MetaCacheConfig {
    pub ttl: u64,               // entry time to live in seconds
}

impl Default for MetaCacheConfig {
    fn default() -> Self {
        MetaCacheConfig {
            ttl: 60,            // 60 c
        }
    }
}
pub struct MetaCache {
    cache: Cache<PathBuf, Meta>,
}

impl MetaCache {
    pub fn new(config: MetaCacheConfig) -> Self {
        let cache = Cache::builder()
            // Max 100,000 entries
            .max_capacity(100_000)
            .time_to_live(Duration::from_secs(config.ttl))
            .build();
        MetaCache { cache }
    }

    pub async fn metadata(&self, path: &PathBuf) -> io::Result<Meta> {
        match self.cache.get(path) {
            Some(meta) => Ok(meta),
            None => {
                let meta = Meta::from_path(path).await?;
                self.cache.insert(path.clone(), meta.clone()).await;
                Ok(meta)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn metadata() {
        let path = PathBuf::from("LICENSE");
        let cache = MetaCache::new(MetaCacheConfig::default());

        let meta1 = Meta::from(tokio::fs::metadata(&path).await.unwrap());
        let meta2 = cache.metadata(&path).await.unwrap();

        assert_eq!(meta1, meta2);

        sleep(Duration::from_millis(100)).await;
        let meta3 = cache.metadata(&path).await.unwrap();

        assert_eq!(meta2, meta3);
    }
}
