use std::{fs, path::Path, time::Duration};

use anyhow::Result;
use cache::{Cache, CacheEntry, Query};
use heed::{
    Database, Env, EnvOpenOptions,
    types::{SerdeJson, Str},
};

pub struct LocalCache {
    env: Env,
    storage: Database<Str, SerdeJson<CacheEntry<Query>>>,
    ttl: Duration,
}

impl LocalCache {
    pub fn new<P: AsRef<Path>>(path: P, ttl: Option<Duration>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(10 * 1024 * 1024)
                .max_dbs(40)
                .open(path.as_ref())?
        };

        let mut wtxn = env.write_txn()?;
        let storage = env.create_database(&mut wtxn, Some("cache"))?;
        wtxn.commit()?;

        Ok(LocalCache {
            env,
            storage,
            ttl: ttl.unwrap_or(Duration::from_secs(60 * 60 * 24)),
        })
    }
}

impl Cache for LocalCache {
    fn store(&self, query: Query) -> Result<()> {
        let mut write_txn = self.env.write_txn()?;
        let key = query.text.clone();
        let entry = CacheEntry {
            created_at: chrono::Utc::now().naive_utc(),
            value: query,
        };
        self.storage.put(&mut write_txn, &key, &entry)?;
        write_txn.commit()?;
        Ok(())
    }

    fn search_similarity(&self, embedding: &[f32]) -> Result<Vec<(Query, f32)>> {
        let (results, keys_to_purge) = {
            let mut read_txn = self.env.read_txn()?;
            let mut results = Vec::new();
            let mut keys_to_purge = Vec::new();
            let now = chrono::Utc::now().naive_utc();

            for item in self.storage.iter(&mut read_txn)? {
                let (key, entry_result) = item?;
                let entry: CacheEntry<Query> = entry_result;

                let entry_age = now - entry.created_at;
                if entry_age > chrono::Duration::from_std(self.ttl).unwrap() {
                    keys_to_purge.push(key.to_owned());
                    continue;
                }

                let query_embedding = &entry.value.embedding;
                let mut dot_product = 0.0;
                let mut query_magnitude = 0.0;
                let mut embedding_magnitude = 0.0;

                for (a, b) in query_embedding.iter().zip(embedding.iter()) {
                    dot_product += a * b;
                    query_magnitude += a * a;
                    embedding_magnitude += b * b;
                }

                query_magnitude = query_magnitude.sqrt();
                embedding_magnitude = embedding_magnitude.sqrt();

                if query_magnitude > 0.0 && embedding_magnitude > 0.0 {
                    let similarity = dot_product / (query_magnitude * embedding_magnitude);

                    results.push((entry.value, similarity));
                }
            }

            results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            (results, keys_to_purge)
        };

        if !keys_to_purge.is_empty() {
            let mut write_txn = self.env.write_txn()?;
            for key in keys_to_purge {
                self.storage.delete(&mut write_txn, &key)?;
            }
            write_txn.commit()?;
        }

        Ok(results)
    }
}
