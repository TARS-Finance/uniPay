use async_trait::async_trait;

#[async_trait]
pub trait Cache<K, V> {
    async fn get(&self, id: &K) -> Option<V>;
    async fn set(&self, items: &[(K, V)]);
    async fn clear(&self);
    async fn keys(&self) -> Vec<K>;
}
