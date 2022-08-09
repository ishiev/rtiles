use bytes::Bytes;
// use dash cache variant to prevent using GC for eviction 
use moka::dash::Cache;

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
    pub size: u64,      // cache size limit in Mbytes
    pub cache_ttl: u64, // cache entry Time To Live
    pub cache_tti: u64, // cache entry Time To Idle (from last request)
}

impl Default for FileCacheConfig {
    fn default() -> Self {
        FileCacheConfig {
            size: 50,               // 50 MB
            cache_ttl: 24 * 3600,   // 24 hours
            cache_tti: 4 * 3600,    // 4 hours
        }
    }
}


pub enum CachedNamedFile {
    File(NamedFile, u64),
    Cached(Box<Content>)
}

impl CachedNamedFile {
    /// Open file and get content size
    pub async fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let f = NamedFile::open(path).await?;
        let l = f.metadata().await?.len();

        Ok(CachedNamedFile::File(f, l))
    }

    /// Get back cached content or open named file
    pub async fn open_with_cache(path: &PathBuf, cache: &FileCache) 
    -> io::Result<Self> {
        // try to get content from cache
        if let Some(cnt) = cache.get(path) {
            Ok(CachedNamedFile::Cached(Box::new(cnt)))
        } else {
            // try to open a file from a given path
            let f = Self::open(path).await?;

            // check file length against cache size and u32::MAX (cache weigher limit )
            let len = f.len();
            if len <= cache.size() && len <= u32::MAX as u64 {
                cache.insert(path).unwrap_or_else(
                    |err| error!("error adding file to cache: {}", err)
                )
            } else {
                warn!(
                    "file {} exceeds cache size or 4GB limit, not cached",
                    path.to_string_lossy()
                )
            } 
            Ok(f)
        }
    }

    /// Get content lenght
    pub fn len(&self) -> u64 {
        match self {
            CachedNamedFile::File(_, l) => *l,
            CachedNamedFile::Cached(c) => c.length
        }
    }

    pub fn is_cached(&self) -> bool {
        match self {
            CachedNamedFile::File(..) => false,
            CachedNamedFile::Cached(_) => true
        }
    }
}

/// Combined responder for named file and cached content
impl<'r> Responder<'r, 'static> for CachedNamedFile {
    fn respond_to(self, req: &'r Request<'_>) -> response::Result<'static> {
        match self {
            CachedNamedFile::File(f, _) => {
                // set content type more properly...
                let mime_type = match f.path().extension() {
                    Some(ext) => ContentType::from_extension(&ext.to_string_lossy()),
                    None => None,
                };
                let mut response = f.take_file().respond_to(req)?;
                response.set_header(mime_type.unwrap_or(ContentType::Binary));
                Ok(response)
            },
            CachedNamedFile::Cached(c) => c.respond_to(req)
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

        // get content length
        let length = f.metadata().await?.len();

        // parse content type from file extension if the extension is
        // recognized. See [`ContentType::from_extension()`] for more information.
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

/// Streams the content to the client
impl<'r> Responder<'r, 'static> for Content {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        Response::build()
            .header(self.mime_type.unwrap_or(ContentType::Binary))
            .sized_body(Some(self.length as usize), Cursor::new(self.body))
            .ok()
    }
}

/// File cache
pub struct FileCache {
    cache: Cache<PathBuf, Content>,
    tx: mpsc::Sender<PathBuf>,
    size: u64
}

impl FileCache {
    pub fn new(config: &FileCacheConfig) -> Self {
        // cache size in bytes
        let size = config.size * 1024 * 1024;
        // build cache
        let cache = Cache::builder()
            // closure to calculate item size
            .weigher(|key: &PathBuf, value: &Content| -> u32 {
                if value.length > u32::MAX as u64 {
                    error!(
                        "file size for caching exceeds 4G! file: {}, size: {}", 
                        key.to_string_lossy(),
                        value.length
                    );
                    u32::MAX
                } else {
                    value.length as u32
                }
            })
            // Max cache size
            .max_capacity(size)
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
                // check cache for the path
                if cache_rx.get(&path).is_some() {
                    // already in cache, skip
                    continue;
                }
                // load content and insert to cache
                match Content::from_file(&path).await {
                    Ok(cnt) => {
                        cache_rx.insert(path, cnt)
                    },
                    Err(err) => {
                        error!("cache file loading error: {}", err)
                    }
                }
            }
            debug!("cache file upload task completed");
        });

        FileCache { cache, tx, size }
    }

    /// Schedule file save to cache
    pub fn insert(&self, path: &Path) 
        -> Result<(), mpsc::error::TrySendError<PathBuf>> {
        // fails if no capacity in the channel
        self.tx.try_send(path.to_path_buf())
    }

    /// Get cached content
    pub fn get(&self, path: &PathBuf) -> Option<Content> {
        self.cache.get(path)
    }

    /// Cache size in bytes
    pub fn size(&self) -> u64 {
        self.size
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
    async fn content_from_file() {
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

        assert_eq!(dst1, dst2);
    }

    #[tokio::test]
    async fn file_cache() {
        let path = PathBuf::from("README.md");

        let cache = FileCache::new(&FileCacheConfig::default());
        cache.insert(&path).unwrap();
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

        assert_eq!(dst1, dst2);
    }

    #[tokio::test]
    async fn cached_named_file() {
        let path = PathBuf::from("README.md");
        let cache = FileCache::new(&FileCacheConfig::default());
        let mut buf = (Vec::new(), Vec::new());
        
        match CachedNamedFile::open_with_cache(&path, &cache).await.unwrap() {
            CachedNamedFile::File(mut f, _) => f
                .read_to_end(&mut buf.0)
                .await
                .unwrap(),
            CachedNamedFile::Cached(_) => panic!("named file expected!")
        };

        // delay before get from cache
        sleep(Duration::from_millis(100)).await;

        match CachedNamedFile::open_with_cache(&path, &cache).await.unwrap() {
            CachedNamedFile::File(..) => panic!("cached expected!"),
            CachedNamedFile::Cached(c) => c.body
                .reader()
                .read_to_end(&mut buf.1)
                .unwrap()
        };

        assert_ne!(buf.0.len(), 0);
        assert_eq!(buf.0, buf.1);
    }
}
