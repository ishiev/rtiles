use std::collections::HashMap;
use std::ops::AddAssign;
use std::sync::Arc;
use tokio::task::{self, JoinHandle};
use tokio::sync::{mpsc, RwLock};

/// Statistic key
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct StatKey {
    object: Option<String>,   // None means aggregates for all models of all objects
    model: Option<String>,    // None means aggregates for all models of a given object
}

impl Default for StatKey {
    fn default() -> Self {
        StatKey {object: None, model: None}
    }   
}

impl StatKey {
    fn new(object: &str, model: &str) -> Self {
        StatKey {object: Some(object.to_owned()), model: Some(model.to_owned())}
    }
}

/// Statistic metrics
#[derive(Debug, Copy, Clone, PartialEq)]
struct Metrics {
    hits: u64,                // request count
    bytes: u64                // request bytes     
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {hits: 0, bytes: 0}
    }
}

impl AddAssign for Metrics {
    // aggregate method for Metrics
    fn add_assign(&mut self, other: Self) {
        *self = Self {
            hits: self.hits + other.hits,
            bytes: self.bytes + other.bytes,
        };
    }
}


/// Statistic record
#[derive(Debug)]
struct Record {
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
    async fn insert(&self, mut rec: Record) {
        // lock map for update
        let mut map = self.0.write().await;

        if rec.key.model.is_some() {
            if rec.key.object.is_none() {
                // illegal model key
                error!("illegal model key for stat insert: {:?}, ignored", &rec.key);
                return;
            }
            let key = StatKey{ 
                object: rec.key.object.clone(),
                model: None
            };
            // update aggregates for all models of a given object
            let metrics = map.entry(key).or_insert(Metrics::default());
            *metrics += rec.metrics;
        }
        else {
            // if model was set to None, also set object to None
            rec.key.object.take();
        }

        if rec.key.object.is_some() {
            let key = StatKey{ 
                object: None,
                model: None
            };
            // update aggregates for all models of all objects
            let metrics = map.entry(key).or_insert(Metrics::default());
            *metrics += rec.metrics;
        }

        // finally update metrics for the given object and model 
        let metrics = map.entry(rec.key).or_insert(Metrics::default());
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
struct Stat {
    all: Arc<StatTable>,
    tx: mpsc::Sender<Record>,
}

impl Stat {
    fn new() -> Self {
        let all = Arc::new(StatTable::new());
        let all_rx = Arc::clone(&all);
        let (tx, mut rx) = mpsc::channel(500);
        
        // spawn a detached async task
        // task ended when the channel has been closed 
        task::spawn(async move {
            debug!("stat recv task started");
            while let Some(rec) = rx.recv().await {
                // insert record to stat table
                all_rx.insert(rec).await;
            }
            debug!("stat recv task finished");
        });

        Stat { all, tx }
    }

    async fn insert(&self, rec: Record) 
        -> Result<(), mpsc::error::SendError<Record>> {
        self.tx.send(rec).await
    }

    async fn get(&self, key: &StatKey) -> Metrics {
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
        let mut key = StatKey::default();
        let metrics = Metrics { hits: 1, bytes: 1000 };
        let stat = StatTable::new();

        // test first model metrics 
        key.object = Some("lake".to_owned());
        key.model  = Some("first".to_owned());
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        let mut res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, bytes: 2000 });

        // test second model metrics
        key.model = Some("second".to_owned());
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 1, bytes: 1000 });

        // test metrics for whole object
        key.model = None;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 3, bytes: 3000 });

        // test another object metrics 
        key.object = Some("land".to_owned());
        key.model  = Some("first".to_owned());
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, bytes: 2000 });

        // test metrics for another whole object
        key.model = None;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 2, bytes: 2000 });

        // test metrics for server
        key.object = None;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 5, bytes: 5000 });

        // test illegal object and model key metrics 
        key.model  = Some("first".to_owned());
        stat.insert(Record { key: key.clone(), metrics }).await;
        stat.insert(Record { key: key.clone(), metrics }).await;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 0, bytes: 0 });

        // again test metrics for server 
        key.model = None;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 5, bytes: 5000 });
    }

    #[tokio::test]
    async fn stat_server() {
        let mut key = StatKey {
            object: Some("city".to_owned()),
            model: Some("block".to_owned())
        };
        let metrics = Metrics { hits: 1, bytes: 1000 };
        let stat = Stat::new();

        for _ in 0..10 {
            stat.insert(Record { key: key.clone(), metrics }).await.unwrap();
        }
        let mut res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 10, bytes: 10000 });

        // test metrics for server
        key.object = None;
        key.model = None;
        res = stat.get(&key).await;
        assert_eq!(res, Metrics { hits: 10, bytes: 10000 });
    }
}