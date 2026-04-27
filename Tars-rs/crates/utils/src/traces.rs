use eyre::{eyre, Result};

pub fn setup_tracing_with_webhook(
    webhook_url: &str,
    _app_name: &str,
    level: tracing::Level,
    _formatter: Option<Box<dyn Send + Sync>>,
) -> Result<()> {
    if !webhook_url.starts_with("http://") && !webhook_url.starts_with("https://") {
        return Err(eyre!("Invalid webhook URL format"));
    }

    tracing_subscriber::fmt()
        .with_max_level(level)
        .json()
        .try_init()
        .map_err(|e| eyre!("Failed to initialize tracing subscriber: {e}"))
}
