use bytes::Bytes;
use moka::future::Cache;

use rocket::fs::NamedFile;
use rocket::http::ContentType;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use rocket::serde::{Deserialize, Serialize};

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::fs::File;
use tokio::io::{self, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task;

/// File cache configuration
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


pub enum CachedNamedFile {
    File(NamedFile),
    Cached(Content)
}

impl CachedNamedFile {
    /// Get back cached content or open named file
    pub async fn open_with_cache(path: PathBuf, cache: &FileCache) 
    -> io::Result<CachedNamedFile> {
        // try to get content from cache
        if let Some(cnt) = cache.get(&path) {
            Ok(CachedNamedFile::Cached(cnt))
        } else {
            let f = NamedFile::open(&path).await;
            if let Ok(_) = f {
                // insert new file to cache only if open is Ok
                cache.insert(path).unwrap_or_else(
                    | err | error!("error insert new path to cache: {}", err)
                )
            }
            f.map(|x| CachedNamedFile::File(x))
        }
    }

    /// Get content lenght
    pub async fn len(&self) -> usize {
        match self {
            CachedNamedFile::File(f) => 
                match f.metadata().await {
                    Ok(m) => m.len() as usize,
                    Err(_) => 0
                }
            CachedNamedFile::Cached(c) => c.length
        }
    }

    pub fn is_cached(&self) -> bool {
        match self {
            CachedNamedFile::File(_) => false,
            CachedNamedFile::Cached(_) => true
        }
    }
}

/// Combined responder for named file and cached content
impl<'r> Responder<'r, 'static> for CachedNamedFile {
    fn respond_to(self, req: &'r Request<'_>) -> response::Result<'static> {
        match self {
            CachedNamedFile::File(f) => f.respond_to(req),
            CachedNamedFile::Cached(c) => c.respond_to(req),
        }
    }
}


/// Saved content
#[derive(Clone)]
pub struct Content {
    length: usize,                  // size in bytes
    mime_type: Option<ContentType>, // content mime type
    body: Bytes,                    // body in-memory buffer
}

impl Content {
    /// Read file to content buffer
    async fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Content> {
        // open file for reading
        let mut f = File::open(&path).await?;

        // get content length
        let length = f.metadata().await?.len() as usize;

        // parse content type from file extension if the extension is
        // recognized. See [`ContentType::from_extension()`] for more information.
        let mime_type = match path.as_ref().extension() {
            Some(ext) => ContentType::from_extension(&ext.to_string_lossy()),
            None => None,
        };
        
        // read the whole file to
        let mut buf = Vec::with_capacity(length);
        let bytes = f.read_to_end(&mut buf).await?;

        assert_eq!(bytes, length);

        Ok(Content {
            length,
            mime_type,
            body: Bytes::from(buf),
        })
    }
}

/// Streams the content to the client
impl<'r> Responder<'r, 'static> for Content {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        Response::build()
            .header(self.mime_type.unwrap_or(ContentType::Binary))
            .sized_body(Some(self.length), Cursor::new(self.body))
            .ok()
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
