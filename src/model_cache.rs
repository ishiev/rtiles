use bytes::Bytes;
use moka::future::Cache;
use rocket::http::ContentType;
use rocket::serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{self, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task;

/// Model file cache configuration
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct FileCacheConfig {
    pub size: u64,      // cache size limit in bytes
    pub cache_ttl: u64, // cache entry Time To Live
    pub cache_tti: u64, // cache entry Time To Idle (from last request)
}

impl Default for FileCacheConfig {
    fn default() -> Self {
        FileCacheConfig {
            size: 50 * 1024 * 1024, // 50 MB
            cache_ttl: 24 * 3600,   // 24 hours
            cache_tti: 4 * 3600,    // 4 hours
        }
    }
}

/// Saved content
#[derive(Clone)]
pub struct Content {
    length: u64,                    // size in bytes
    mime_type: Option<ContentType>, // content mime type
    body: Bytes,                    // body in-memory buffer
}

impl Content {
    /// Read file to content buffer
    async fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Content> {
        // open file for reading
        let mut f = File::open(&path).await?;

        // get file length
        let length = f.metadata().await?.len();

        // parse content type from file extension
        let mime_type = match path.as_ref().extension() {
            Some(ext) => ContentType::from_extension(&ext.to_string_lossy()),
            None => None,
        };
        
        // read the whole file to
        let mut buf = Vec::with_capacity(length as usize);
        let bytes = f.read_to_end(&mut buf).await?;

        assert_eq!(bytes as u64, length);

        Ok(Content {
            length,
            mime_type,
            body: Bytes::from(buf),
        })
    }
}


/// File cache
pub struct FileCache {
    cache: Cache<PathBuf, Content>,
    tx: mpsc::Sender<PathBuf>,
}

impl FileCache {
    fn new(config: &FileCacheConfig) -> Self {
        // build cache
        let cache = Cache::builder()
            // closure to calculate item size
            .weigher(|_key, value: &Content| -> u32 {
                value.length as u32
            })
            .max_capacity(config.size)
            // Max TTL for items
            .time_to_live(Duration::from_secs(config.cache_ttl))
            // Max TTI for items - 5 min
            .time_to_idle(Duration::from_secs(config.cache_tti))
            .build();

        // share same cache with the detached task (this is cheap operation)
        let cache_rx = cache.clone();
        let (tx, mut rx) = mpsc::channel(500);
        
        // spawn a detached async task 
        // task ended when the channel has been closed 
        task::spawn(async move {
            while let Some(path) = rx.recv().await {
                // load file from disk and insert to cache
                match Content::from_file(&path).await {
                    Ok(cnt) => {
                        cache_rx.insert(path, cnt).await
                    },
                    Err(err) => {
                        // no inserts if problem with file loading
                        error!("cache file loading error: {}", err)
                    }
                }
            }
            debug!("cache file upload task completed");
        });

        FileCache { cache, tx }
    }

    pub fn insert(&self, path: PathBuf) 
        -> Result<(), mpsc::error::TrySendError<PathBuf>> {
        // fails if no capacity in the channel
        self.tx.try_send(path)

        // alternative: not use channel, spawn a detached async task for each insert 
        // and return task handle with result (content size or error)...
    }

    pub fn get(&self, path: &PathBuf) -> Option<Content> {
        self.cache.get(path)
    }
}


#[cfg(test)]
mod test {
    use super::*;
    use bytes::Buf;
    use std::fs::File;
    use std::io::Read;
    use tokio::time::sleep;

    #[tokio::test]
    async fn load_file() {
        let path = "README.md";

        let cnt = Content::from_file(path).await.unwrap();
        println!(
            "{} bytes read, type: {:?}",
            cnt.length,
            cnt.mime_type,
        );

        let mut r = cnt.body.reader();
        let mut dst1 = Vec::new();
        r.read_to_end(&mut dst1).unwrap();

        let mut f = File::open(path).unwrap();
        let mut dst2 = Vec::new();
        f.read_to_end(&mut dst2).unwrap();

        assert_eq!(&dst1, &dst2);
    }

    #[tokio::test]
    async fn file_cache() {
        let path = PathBuf::from("README.md");

        let cache = FileCache::new(&FileCacheConfig::default());
        cache.insert(path.clone()).unwrap();
        // ...starting async file reading...
        // delay before get back content
        sleep(Duration::from_millis(100)).await;
        let cnt = cache.get(&path).unwrap();

        let mut r = cnt.body.reader();
        let mut dst1 = Vec::new();
        r.read_to_end(&mut dst1).unwrap();

        let mut f = File::open(path).unwrap();
        let mut dst2 = Vec::new();
        f.read_to_end(&mut dst2).unwrap();

        assert_eq!(&dst1, &dst2);
    }
}
