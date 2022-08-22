use std::collections::HashMap;
use std::ops::AddAssign;
use std::sync::Arc;
use tokio::task;
use tokio::sync::{mpsc, RwLock};
use serde::Serialize;

use crate::Model;

/// Statistic key
#[derive(Default, Debug, Clone, Hash, PartialEq, Eq)]
pub struct StatKey {
    pub model: Arc<Model>
}

impl StatKey {
    pub fn new(object: Option<&str>, name: Option<&str>) -> Self {
        StatKey { 
            model: Arc::new(Model::new(object, name))
        }
    }
}


/// Statistic metrics
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub hits: u64,                // request count
    pub cached: u64,              // cached request count
    pub bytes: u64                // request bytes     
}

impl AddAssign for Metrics {
    // aggregate method for Metrics
    fn add_assign(&mut self, other: Self) {
        *self = Self {
            hits: self.hits + other.hits,
            cached: self.cached + other.cached,
            bytes: self.bytes + other.bytes,
        };
    }
}

/// Statistic record
#[derive(Debug)]
pub struct Record {
    key: StatKey,
    metrics: Metrics
}

/// Async in-memory stitistic table
struct StatTable(RwLock<HashMap<StatKey, Metrics>>);

impl StatTable {
    /// Create empty table
    fn new() -> Self {
        StatTable(RwLock::new(HashMap::new()))
    }

    /// Insert new metrics, calculate aggregates
    async fn insert(&self, rec: Record) {
        // lock map for update
        let mut map = self.0.write().await;

        if rec.key.model.name.is_some() {
            if rec.key.model.object.is_none() {
                // illegal model key
                error!("illegal model key for stat insert: {:?}, ignored", &rec.key);
                return;
            }
            let key = StatKey::new(
                rec.key.model.object.as_deref(), 
                None
            );
            // update aggregates for all models of a given object
            let metrics = map.entry(key).or_insert_with(Metrics::default);
            *metrics += rec.metrics;
        }
        else {
            // if model was set to None, also set object to None
            // NOTE: mutate Arc'ed model shared reference
            //cargorec.key.model.object.take();
        }

        if rec.key.model.object.is_some() {
            let key = StatKey::new(None, None);
            // update aggregates for all models of all objects
            let metrics = map.entry(key).or_insert_with(Metrics::default);
            *metrics += rec.metrics;
        }

        // finally update metrics for the given object and model 
        let metrics = map.entry(rec.key).or_insert_with(Metrics::default);
        *metrics += rec.metrics;
    }

    /// Get metrics by the key
    async fn get(&self, key: &StatKey) -> Metrics {
        // shared lock map for read
        let map = self.0.read().await;
        match map.get(key) {
            Some(metrics) => *metrics,
            None => Metrics::default()
        }
    }
}


/// Server statistics
#[derive(Clone)]
pub struct Stat {
    all: Arc<StatTable>,
    tx: mpsc::Sender<Record>,
}

impl Stat {
    pub fn new() -> Self {
        let all = Arc::new(StatTable::new());
        let all_rx = Arc::clone(&all);
        let (tx, mut rx) = mpsc::channel(500);
        
        // spawn a detached async task
        // task ended when the channel has been closed 
        task::spawn(async move {
            while let Some(rec) = rx.recv().await {
                // insert record to stat table
                all_rx.insert(rec).await;
            }
            debug!("stat recv task finished");
        });

        Stat { all, tx }
    }

    pub async fn insert(&self, key: StatKey, metrics: Metrics) 
        -> Result<(), mpsc::error::SendError<Record>> {
        self.tx.send(Record{ key, metrics }).await
    }

    pub async fn get(&self, key: &StatKey) -> Metrics {
        // move current task to end of the task queue 
        // to complete async inserts before get back values
        task::yield_now().await;
        self.all.get(key).await
    }
}


#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn stat_table() {
        let metrics = Metrics { hits: 1, cached: 1,  bytes: 1000 };
        let stat = StatTable::new();
        let mut key;

        // test first model metrics 
        key = StatKey::new(Some("lake"), Some("first"));
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        let mut res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, cached: 2, bytes: 2000 });

        // test second model metrics
        key = StatKey::new(Some("lake"), Some("second"));
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 1, cached: 1, bytes: 1000 });

        // test metrics for whole object
        key = StatKey::new(Some("lake"), None);
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 3, cached: 3, bytes: 3000 });

        // test another object metrics 
        key = StatKey::new(Some("land"), Some("first"));
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, cached: 2, bytes: 2000 });

        // test metrics for another whole object
        key = StatKey::new(Some("land"), None);
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, cached: 2, bytes: 2000 });

        // test metrics for server
        key = StatKey::default();
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 5, cached: 5, bytes: 5000 });

        // test illegal object and model key metrics 
        key = StatKey::new(None, Some("first"));
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 0, cached: 0, bytes: 0 });

        // again test metrics for server 
        key = StatKey::default();
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 5, cached: 5, bytes: 5000 });
    }

    #[tokio::test]
    async fn stat_server() {
        let mut key = StatKey::new (
            Some("city"),
            Some("block")
        );
        let metrics = Metrics { hits: 1, cached: 1, bytes: 1000 };
        let stat = Stat::new();

        for _ in 0..10 {
            stat.insert(key.clone(), metrics).await.unwrap();
        }
        let mut res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 10, cached: 10, bytes: 10000 });

        // test metrics for server
        key = StatKey::default();
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 10, cached: 10, bytes: 10000 });
    }
}