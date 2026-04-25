use crate::core::Cache;
use async_trait::async_trait;
use moka::future::Cache as MokaCache;
use std::hash::Hash;
use std::time::Duration;

pub struct MokaCacheAdaptor<K, V>
where
    K: Hash + Eq + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
{
    cache: MokaCache<K, V>,
}

impl<K, V> MokaCacheAdaptor<K, V>
where
    K: Hash + Eq + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            cache: MokaCache::builder().build(),
        }
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: MokaCache::builder().time_to_live(ttl).build(),
        }
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for MokaCacheAdaptor<K, V>
where
    K: Hash + Eq + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn get(&self, id: &K) -> Option<V> {
        self.cache.get(id).await
    }

    async fn set(&self, items: &[(K, V)]) {
        for (key, value) in items {
            self.cache.insert(key.clone(), value.clone()).await;
        }
    }

    async fn clear(&self) {
        self.cache.invalidate_all();
    }

    async fn keys(&self) -> Vec<K> {
        self.cache.iter().map(|(k, _)| (*k).clone()).collect()
    }
}
