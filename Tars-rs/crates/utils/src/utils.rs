//! Common utils for unipay rust apps

use alloy::{
    hex::FromHex,
    primitives::{Bytes, FixedBytes},
};
use base64::Engine;

use aes_gcm::{
    aead::{Aead, Nonce},
    AeadCore, Aes256Gcm, KeyInit,
};
use alloy::hex;
use core::fmt;
use eyre::{eyre, Result};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use rand::rngs::OsRng;
use rand::Rng;
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer,
};
use sha2::Digest;
use std::{
    collections::{HashMap, HashSet},
    env,
    future::Future,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::{runtime::Handle, time::sleep};
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, Layer};
use url::Url;

/// Trait that provides functionality to convert hexadecimal strings to bytes.
///
/// This trait extends types with the ability to convert from a hexadecimal representation
/// (with or without "0x" prefix) into a byte array.
pub trait ToBytes {
    /// Converts a hexadecimal string to bytes.
    ///
    /// # Returns
    /// * `Result<Bytes>` - The bytes representation or an error if conversion fails
    fn hex_to_bytes(&self) -> eyre::Result<Bytes>;

    /// Converts a hexadecimal string to FixedBytes<N>.
    ///
    /// # Returns
    /// * `Result<FixedBytes<N>>` - The fixed bytes representation or an error if conversion fails
    fn hex_to_fixed_bytes(&self) -> eyre::Result<FixedBytes<32>>;
}

impl ToBytes for &str {
    fn hex_to_bytes(&self) -> eyre::Result<Bytes> {
        // Strip the 0x prefix if present
        let s = self.strip_prefix("0x").unwrap_or(self);
        // Convert the hex string to bytes, propagating any errors
        Bytes::from_hex(s).map_err(|e| eyre::eyre!("Failed to convert hex to bytes: {}", e))
    }

    fn hex_to_fixed_bytes(&self) -> eyre::Result<FixedBytes<32>> {
        let s = self.strip_prefix("0x").unwrap_or(self);
        FixedBytes::<32>::from_hex(s)
            .map_err(|e| eyre::eyre!("Failed to convert hex to FixedBytes<{}>: {}", 32, e))
    }
}

impl ToBytes for String {
    fn hex_to_bytes(&self) -> eyre::Result<Bytes> {
        self.as_str().hex_to_bytes()
    }

    fn hex_to_fixed_bytes(&self) -> eyre::Result<FixedBytes<32>> {
        self.as_str().hex_to_fixed_bytes()
    }
}

/// Trait that enables SHA-256 hashing functionality.
///
/// This trait extends types with the ability to compute their SHA-256 hash.
pub trait Hashable {
    /// Computes the SHA-256 hash of the string.
    ///
    /// # Returns
    /// * `Result<FixedBytes<32>>` - The 32-byte SHA-256 hash or an error
    fn sha256(&self) -> eyre::Result<FixedBytes<32>>;
}

impl Hashable for String {
    fn sha256(&self) -> eyre::Result<FixedBytes<32>> {
        let mut hasher = sha2::Sha256::default();
        hasher.update(self.as_bytes());
        Ok(FixedBytes::new(hasher.finalize().into()))
    }
}

impl Hashable for &str {
    fn sha256(&self) -> eyre::Result<FixedBytes<32>> {
        let mut hasher = sha2::Sha256::default();
        hasher.update(self.as_bytes());
        Ok(FixedBytes::new(hasher.finalize().into()))
    }
}

/// Deserializes a string that may contain an environment variable reference.
///
/// This function is designed to be used with Serde's deserialization process to
/// support environment variable interpolation in configuration files. When a field
/// in a configuration file is prefixed with "#ENV:", the rest of the string is treated
/// as an environment variable name, and its value is substituted.
///
/// When a field is prefixed with "#EncryptedENV:", the rest of the string is treated
/// as an encrypted value that needs to be decrypted before use. Currently, the system uses AES-256-GCM
/// encryption for secure storage of sensitive configuration.
///
/// # Format
/// The expected format is: `#EncryptedENV:hex_encoded_encrypted_data`
///
/// # Requirements
/// - Requires the `AES_SECRET_KEY` environment variable to be set with a valid 64-character
///   hex-encoded key (representing a 32-byte AES-256 key) in the environment
/// - The encrypted value must be properly formatted as hex-encoded string containing:
///   - A 12-byte nonce prefix followed by the actual ciphertext
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Config {
///     #[serde(deserialize_with = "utils::deserialize_env_field")]
///     api_key: String,
///     private_key: String,
/// }
///
/// // With a config.json containing:
/// {
///     "api_key": "#ENV:API_KEY",
///     "private_key": "#EncryptedENV:SERVICE_PRIVATE_KEY"
/// }
/// // The API_KEY and SERVICE_PRIVATE_KEY environment variable values will be used instead
/// // AES_SECRET_KEY to be set in the environment where the value is 64 characters long hex string. (32bytes)
/// // AES_SECRET_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
/// ```
///
/// # Parameters
///
/// * `deserializer`: The deserializer that provides the string value
///
/// # Returns
///
/// * `Result<String, D::Error>`: Either the deserialized string or the environment
///   variable value (if the string starts with "#ENV:") or (the decrypted environment
///   variable value if the string starts with "#EncryptedENV:")
pub fn deserialize_env_field<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    // Define a visitor that understands environment variable references
    struct EnvVarVisitor;

    impl<'de> Visitor<'de> for EnvVarVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter
                .write_str("a string or environment variable reference (starting with \"#ENV:\")")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // Check for the environment variable prefix
            if let Some(env_var_name) = value.strip_prefix("#ENV:") {
                // If the string starts with "#ENV:", look up the environment variable
                env::var(env_var_name).map_err(|err| {
                    de::Error::custom(format!(
                        "Environment variable '{}' not found: {}",
                        env_var_name, err
                    ))
                })
            } else if let Some(env_var_name) = value.strip_prefix("#EncryptedENV:") {
                let encrypted_data = env::var(env_var_name).map_err(|err| {
                    de::Error::custom(format!(
                        "Environment variable {} not found: {}",
                        env_var_name, err
                    ))
                })?;

                let secret_key = env::var("AES_SECRET_KEY").map_err(|err| {
                    de::Error::custom(format!(
                        "Environment variable AES_SECRET_KEY not found: {}",
                        err
                    ))
                })?;

                let cipher = Cipher::from_key(&secret_key).map_err(|err| {
                    de::Error::custom(format!("Failed to create cipher from key: {}", err))
                })?;

                let decrypted_bytes = cipher
                    .decrypt(encrypted_data)
                    .map_err(|err| de::Error::custom(format!("Failed to decrypt data: {}", err)))?;

                String::from_utf8(decrypted_bytes).map_err(|err| {
                    de::Error::custom(format!(
                        "Failed to convert decrypted data bytes to string: {}",
                        err
                    ))
                })
            } else {
                // Otherwise return the string as-is
                Ok(value.to_string())
            }
        }

        // Support string values too, not just literals
        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            self.visit_str(&value)
        }
    }

    deserializer.deserialize_str(EnvVarVisitor)
}

/// Deserializes a CSV string into a vector of values.
///
/// This function is designed to be used with Serde's deserialization process to
/// support CSV string interpolation in configuration files. When a field
/// in a configuration file is prefixed with "#CSV:", the rest of the string is treated
/// as a CSV string, and its values are substituted.
///
/// # Format
/// The expected format is: Option<`#CSV:value1,value2,value3`>
///
/// # Parameters
///
/// * `deserializer`: The deserializer that provides the string value
///
/// # Returns
///
/// * `Result<Option<HashSet<T>>, D::Error>`: Either the deserialized HashSet or an error
///   if the string is not a valid CSV
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Config {
///     #[serde(deserialize_with = "utils::deserialize_csv")]
///     values: Option<HashSet<String>>,
/// }
///
/// // With a config.json containing:
/// {
///     "values": "#CSV:value1,value2,value3"
/// }
/// // The values will be deserialized into a HashSet of strings
/// ```
///
/// # Notes
///
/// - The CSV string must be properly formatted with values separated by commas
/// - The values must be able to be parsed into the desired type `T`
/// - The function returns an `Option<HashSet<T>>` to handle cases where the CSV string is optional
/// - The function uses the `std::str::FromStr` trait to parse the values
/// - The function uses the `serde::Deserialize` trait to handle the CSV string
pub fn deserialize_csv_field<'de, D, T>(deserializer: D) -> Result<Option<HashSet<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Eq + std::hash::Hash + serde::Deserialize<'de>,
    <T as FromStr>::Err: fmt::Display,
{
    // Deserialize as Option<String> first
    let opt = Option::<String>::deserialize(deserializer)?;

    match opt {
        Some(s) => {
            let items = s
                .split(',')
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .map(|v| v.parse::<T>().map_err(de::Error::custom))
                .collect::<Result<HashSet<_>, _>>()?; // directly collect into HashSet
            Ok(Some(items))
        }
        None => Ok(None),
    }
}

/// A tracing layer that sends log events to a webhook based on configured level.
///
/// This layer captures events from the tracing system and forwards them
/// to a webhook URL based on the configured level. All events at or below the specified
/// level will be sent. It's designed to provide notification of important events
/// in production environments by posting formatted details to a Discord channel.
///
/// # Features
/// - Configurable log level threshold
/// - Formats log events with timestamp, target, and all recorded fields
/// - Sends asynchronously without blocking the application
/// - Uses a shared HTTP client for connection pooling
#[derive(Clone)]
pub struct WebhookLayer {
    webhook_url: String,
    http_client: reqwest::Client,
    name: String,
    level: tracing::Level,
    formatter: Arc<dyn Fn(&Event, &FieldVisitor, &str) -> serde_json::Value + Send + Sync>,
}

/// Field visitor that collects field values from tracing events.
///
/// This visitor implements the tracing `Visit` trait to collect field values
/// from log events into a hashmap for later formatting and display.
pub struct FieldVisitor {
    fields: HashMap<String, String>,
}

impl FieldVisitor {
    /// Creates a new empty field visitor.
    fn new() -> Self {
        Self {
            fields: HashMap::with_capacity(10), // Pre-allocate with reasonable capacity
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{:?}", value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    // Additional implementations for other field types to improve data capture quality
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .insert(field.name().to_string(), format!("Error: {}", value));
    }
}

/// Configuration for OpenTelemetry log export. URL may include Basic Auth credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct OtelTracingConfig {
    /// OTLP endpoint URL (may include `username:password@` for Basic Auth).
    pub url: String,
    /// Service name for `service.name` resource attribute.
    pub service_label: String,
}

/// Parameters for multi-output tracing: console (always), OpenTelemetry (optional), Discord webhook (optional).
#[derive(Clone)]
pub struct TracingParams {
    /// Service name for logs and webhook notifications.
    pub service_name: String,
    /// Minimum log level for console and OpenTelemetry output.
    pub level: tracing::Level,
    /// Optional OpenTelemetry export config.
    pub otel_config: Option<OtelTracingConfig>,
    /// Optional Discord webhook URL (ERROR events only).
    pub discord_webhook_url: Option<String>,
}

/// Sets up tracing with JSON console output, optional OpenTelemetry export, and optional Discord webhook.
pub fn setup_tracing(params: &TracingParams) -> Result<()> {
    let level_filter = tracing_subscriber::filter::LevelFilter::from_level(params.level);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_filter(level_filter);

    let otel_logs_layer = params
        .otel_config
        .as_ref()
        .map(|otel_config| -> Result<_> {
            let url = Url::parse(&otel_config.url)
                .map_err(|e| eyre!("Invalid OpenTelemetry URL: {}", e))?;
            let username = url.username();
            let password = url.password().unwrap_or_default();

            let endpoint = format!(
                "{}://{}{}{}",
                url.scheme(),
                url.host_str().unwrap_or(""),
                url.port().map(|p| format!(":{}", p)).unwrap_or_default(),
                url.path()
            );

            let exporter = opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .with_http_client(reqwest::Client::new())
                .with_endpoint(endpoint)
                .with_headers(HashMap::from([(
                    "Authorization".to_string(),
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(format!("{}:{}", username, password))
                    ),
                )]))
                .build()
                .map_err(|e| eyre!("Failed to build OpenTelemetry log exporter: {}", e))?;

            let logger_provider = opentelemetry_sdk::logs::LoggerProvider::builder()
                .with_resource(opentelemetry_sdk::Resource::new(vec![
                    KeyValue::new("service.name", otel_config.service_label.clone()),
                    KeyValue::new("service_name", otel_config.service_label.clone()),
                ]))
                .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
                .build();

            Ok(
                opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                    &logger_provider,
                )
                .with_filter(level_filter),
            )
        })
        .transpose()?;

    let webhook_layer = params
        .discord_webhook_url
        .as_ref()
        .map(|webhook_url| {
            WebhookLayer::new(
                webhook_url,
                &params.service_name,
                tracing::Level::ERROR,
                default_message_formatter,
            )
        })
        .transpose()?;

    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otel_logs_layer)
        .with(webhook_layer)
        .try_init()
        .map_err(|e| eyre!("Failed to initialize tracing subscriber: {}", e))
}

impl<S: Subscriber> Layer<S> for WebhookLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Early return if the event level is above the configured level
        if *event.metadata().level() > self.level {
            return;
        }

        // Collect all fields from the event
        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        let message = (self.formatter)(event, &visitor, &self.name);

        let webhook_url = self.webhook_url.clone();
        let client = self.http_client.clone();

        // Spawning the webhook sending as a non-blocking task
        let rt = Handle::current();
        rt.spawn(async move {
            // Use a timeout to prevent hanging if the webhook is slow to respond
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                client.post(webhook_url.as_str()).json(&message).send(),
            )
            .await;

            match result {
                Ok(Ok(response)) => {
                    if !response.status().is_success() {
                        eprintln!("Webhook error: HTTP {}", response.status());
                    }
                }
                Ok(Err(e)) => eprintln!("Failed to send to webhook: {}", e),
                Err(_) => eprintln!("Webhook request timed out"),
            }
        });
    }
}

impl WebhookLayer {
    /// Creates a new webhook layer with optional rate limiting.
    ///
    /// # Arguments
    ///
    /// * `webhook_url` - The webhook URL to send events to
    /// * `name` - Name to display for the webhook message
    /// * `level` - The tracing level threshold for sending events
    /// * `formatter` - Custom formatter function for webhook messages
    ///
    /// # Returns
    ///
    /// A new `WebhookLayer` instance or an error if initialization fails
    ///
    /// # Examples
    ///
    /// ```rust
    /// use tracing::Level;
    /// use utils::WebhookLayer;
    /// use utils::default_message_formatter;
    ///
    /// // Without rate limiting
    /// let discord_layer = WebhookLayer::new(
    ///     "https://discord.com/api/webhooks/...",
    ///     "MyApp",
    ///     Level::ERROR,
    ///     |event, visitor, name| {
    ///         // Custom formatting logic
    ///         serde_json::json!({
    ///             "content": format!("Error in {}", event.metadata().target())
    ///         })
    ///     }
    /// ).unwrap();
    ///
    /// let discord_layer = WebhookLayer::new(
    ///     "https://discord.com/api/webhooks/...",
    ///     "MyApp",
    ///     Level::ERROR,
    ///     default_message_formatter
    /// ).unwrap();
    /// ```
    pub fn new<F>(
        webhook_url: &str,
        name: &str,
        level: tracing::Level,
        formatter: F,
    ) -> Result<Self>
    where
        F: Fn(&Event, &FieldVisitor, &str) -> serde_json::Value + Send + Sync + 'static,
    {
        // Validate the webhook URL format
        if !webhook_url.starts_with("https://") {
            return Err(eyre::eyre!("Invalid webhook URL format"));
        }

        // HTTP client with timeout and user agent
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Rust-WebhookLayer/1.0")
            .build()
            .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            webhook_url: webhook_url.to_string(),
            http_client: client,
            name: name.to_string(),
            level,
            formatter: Arc::new(formatter),
        })
    }
}

/// Default message formatter
///
/// Creates a standardized Discord-friendly error message
pub fn default_message_formatter(
    event: &Event,
    visitor: &FieldVisitor,
    name: &str,
) -> serde_json::Value {
    let timestamp = chrono::Utc::now().to_rfc3339();
    let target = event.metadata().target();
    let target_parts: Vec<&str> = target.split(':').collect();
    let short_target = target_parts.last().unwrap_or(&target);

    let mut content = String::new();
    content.push_str(&format!("Target: {}\n", short_target));

    // Add location info if available
    if let Some(file) = event.metadata().file() {
        if let Some(line) = event.metadata().line() {
            content.push_str(&format!("Location: {}:{}\n", file, line));
        }
    }

    for (key, value) in &visitor.fields {
        content.push_str(&format!("{}: {}\n", key, value));
    }

    let color = match *event.metadata().level() {
        tracing::Level::ERROR => 0xFF0000, // Red
        tracing::Level::WARN => 0xFFA500,  // Orange
        tracing::Level::INFO => 0x00FF00,  // Green
        tracing::Level::DEBUG => 0x808080, // Gray
        tracing::Level::TRACE => 0x0000FF, // Blue
    };

    serde_json::json!({
        "username": name,
        "embeds": [{
            "title": format!("{} event from {}", event.metadata().level().to_string(), name),
            "description": content,
            "color": color,
            "timestamp": timestamp
        }]
    })
}

/// Sets up tracing with a webhook for notifications.
///
/// This function configures a tracing subscriber with:
/// 1. A standard formatting layer for console output (INFO level and above)
/// 2. A webhook layer for sending events at the specified level
///
/// # Arguments
///
/// * `webhook_url` - The webhook URL to send events to
/// * `app_name` - Name to display for the webhook messages
/// * `level` - The level of events to send to the webhook (e.g., ERROR)
/// * `rate_limit_per_minute` - Optional rate limit for webhook messages
/// * `formatter` - Optional custom formatter function for webhook messages
///
/// # Returns
///
/// `Result<()>` - Ok if initialization succeeded, or an error
///
/// # Examples
///
/// ```no_run
/// use tracing::Level;
/// let webhook_url = std::env::var("WEBHOOK_URL")
///     .unwrap_or_else(|_| "https://discord.com/api/webhooks/your/url".to_string());
///
/// utils::setup_tracing_with_webhook(
///     &webhook_url,
///     "MyApp",
///     Level::ERROR,
///     None
/// ).unwrap();
///
/// // Now events at ERROR level will be sent to the webhook
/// tracing::error!("This will appear in the webhook");
/// ```
pub fn setup_tracing_with_webhook(
    webhook_url: &str,
    app_name: &str,
    level: tracing::Level,
    formatter: Option<Box<dyn Fn(&Event, &FieldVisitor, &str) -> serde_json::Value + Send + Sync>>,
) -> Result<()> {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);

    use tracing_subscriber::prelude::*;

    let webhook_layer = WebhookLayer::new(
        webhook_url,
        app_name,
        level,
        match formatter {
            Some(f) => f,
            None => Box::new(default_message_formatter),
        },
    )?;

    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(webhook_layer);

    subscriber
        .try_init()
        .map_err(|e| eyre::eyre!("Failed to initialize tracing: {}", e))
}

/// Generates a random 32-byte secret using the system's secure random number generator
///
/// # Returns
///
/// * `[u8; 32]` - A 32-byte array containing cryptographically secure random values
///
/// # Example
///
/// ```
/// use utils::gen_secret;
/// let (secret, hash) = gen_secret();
/// assert_eq!(secret.len(), 32);
/// ```
pub fn gen_secret() -> (Bytes, FixedBytes<32>) {
    let secret = rand::thread_rng().gen::<[u8; 32]>();
    let x = sha2::Sha256::digest(secret);
    (Bytes::from(secret), FixedBytes::new(x.into()))
}

pub struct Cipher(Aes256Gcm);

/// Trait for types that can be decoded into a byte vector.
///
/// # Examples
///
/// ```
/// use utils::CipherText;
///
/// // String implementation
/// let valid_hex = "48656c6c6f".to_string(); // "Hello" in hex
/// let decoded = valid_hex.decode().unwrap();
/// assert_eq!(decoded, b"Hello");
///
/// // Vec<u8> implementation
/// let data = vec![1, 2, 3, 4, 5];
/// let decoded = data.decode().unwrap();
/// assert_eq!(decoded, data);
/// ```
pub trait CipherText {
    fn decode(&self) -> Result<Vec<u8>>;
}

impl CipherText for String {
    fn decode(&self) -> Result<Vec<u8>> {
        hex::decode(self).map_err(|_| eyre!("Invalid hex string"))
    }
}

impl CipherText for Vec<u8> {
    fn decode(&self) -> Result<Vec<u8>> {
        Ok(self.clone())
    }
}

impl Cipher {
    /// Creates a new Cipher from a hex-encoded 32-byte key (64 hex chars).
    ///
    /// # Examples
    ///
    /// ```
    /// use utils::Cipher;
    ///
    /// // Valid key
    /// let valid_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    /// let cipher = Cipher::from_key(valid_key).unwrap();
    ///
    /// // Invalid key (not hex)
    /// let invalid_key = "not_a_valid_hex_string_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    /// let result = Cipher::from_key(invalid_key);
    /// assert!(result.is_err());
    /// ```
    pub fn from_key(secret_key: &str) -> Result<Self> {
        if secret_key.len() != 64 {
            return Err(eyre!("Invalid key length"));
        }

        let key = hex::decode(secret_key).map_err(|_| eyre!("Invalid key"))?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| eyre!("Invalid key"))?;
        Ok(Self(cipher))
    }

    /// Encrypts the given plaintext, producing a vector containing the nonce followed by the ciphertext.
    ///
    /// # Examples
    ///
    /// ```
    /// use utils::Cipher;
    ///
    /// let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    /// let cipher = Cipher::from_key(key).unwrap();
    /// let plaintext = b"Hello, world!";
    ///
    /// let encrypted = cipher.encrypt(plaintext).unwrap();
    /// assert!(encrypted.len() > plaintext.len()); // Encryption adds the nonce and maybe padding
    /// ```
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .0
            .encrypt(&nonce, plaintext)
            .map_err(|e| eyre!("Encryption failed: {e}"))?;
        let mut output = Vec::with_capacity(nonce.len() + ciphertext.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    /// Decrypts the given ciphertext, which should be a nonce followed by encrypted data.
    /// Accepts any type that implements CipherText trait.
    ///
    /// # Examples
    ///
    /// ```
    /// use utils::Cipher;
    /// use alloy::hex;
    ///
    /// let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    /// let cipher = Cipher::from_key(key).unwrap();
    /// let plaintext = b"Hello, world!";
    ///
    /// // Basic encrypt/decrypt example
    /// let encrypted = cipher.encrypt(plaintext).unwrap();
    /// let decrypted = cipher.decrypt(encrypted).unwrap();
    /// assert_eq!(decrypted, plaintext);
    ///
    /// // Decrypt from hex string
    /// let plaintext = b"Test with string ciphertext";
    /// let encrypted = cipher.encrypt(plaintext).unwrap();
    /// let encrypted_hex = hex::encode(&encrypted);
    /// let decrypted = cipher.decrypt(encrypted_hex).unwrap();
    /// assert_eq!(decrypted, plaintext);
    /// ```
    pub fn decrypt<T: CipherText>(&self, ciphertext: T) -> Result<Vec<u8>> {
        let decoded = ciphertext.decode()?;
        if decoded.len() <= 12 {
            return Err(eyre!("Encrypted data too short"));
        }
        let (nonce_bytes, ciphertext) = decoded.split_at(12);
        let nonce = Nonce::<Aes256Gcm>::from_slice(nonce_bytes);
        let plaintext = self
            .0
            .decrypt(nonce, ciphertext)
            .map_err(|e| eyre!("Decryption failed: {e}"))?;
        Ok(plaintext)
    }
}

/// Retry with backoff
///
/// This function retries an operation with backoff.
/// It includes the operation, the max retries, and the initial delay.
///
/// # Arguments
///
/// * `operation` - The operation to retry
/// * `max_retries` - The maximum number of retries
/// * `initial_delay_ms` - The initial delay in milliseconds
///
/// # Returns
///
/// * `Result<T, E>` - The result of the operation
pub async fn retry_with_backoff<F, Fut, T, E>(
    mut operation: F,
    max_retries: usize,
    initial_delay_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    for attempt in 0..max_retries - 1 {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::error!("{}", e);
                let delay = initial_delay_ms * (2_u64.pow(attempt as u32));
                sleep(Duration::from_millis(delay)).await;
            }
        }
    }
    operation().await
}

/// A wrapper struct for a vector that guarantees at least one element
#[derive(Debug, Clone)]
pub struct NonEmptyVec<T>(Vec<T>);

impl<T> NonEmptyVec<T> {
    /// Creates a new NonEmptyVec from a vector
    /// # Arguments
    /// * `vec` - Input vector to wrap
    /// # Returns
    /// * `Ok(NonEmptyVec<T>)` if the vector is non-empty
    /// * `Err(eyre::Report)` if the vector is empty
    pub fn new(vec: Vec<T>) -> Result<Self, eyre::Report> {
        Self::try_from(vec)
    }

    /// Returns a reference to the inner vector
    /// # Returns
    /// A reference to the underlying Vec<T>
    pub fn as_ref(&self) -> &Vec<T> {
        &self.0
    }

    /// Returns a mutable reference to the inner vector
    /// # Returns
    /// A mutable reference to the underlying Vec<T>
    pub fn as_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }

    /// Returns the first element of the vector
    /// # Returns
    /// A reference to the first element
    /// # Safety
    /// Guaranteed to return a value since the vector is non-empty
    pub fn first(&self) -> &T {
        self.0.first().expect("NonEmptyVec is never empty")
    }

    /// Returns the length of the inner vector
    /// # Returns
    /// The number of elements in the vector (always >= 1)
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<T> TryFrom<Vec<T>> for NonEmptyVec<T> {
    type Error = eyre::Report;

    /// Attempts to create a NonEmptyVec from a vector
    /// # Arguments
    /// * `vec` - The input vector to convert
    /// # Returns
    /// * `Ok(NonEmptyVec<T>)` if the vector is non-empty
    /// * `Err(eyre::Report)` if the vector is empty
    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        if vec.is_empty() {
            eyre::bail!("expected a non-empty vector");
        }
        Ok(Self(vec))
    }
}

// Implementing Deref for convenient access to Vec methods
impl<T> std::ops::Deref for NonEmptyVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// Implementing DerefMut for convenient mutable access to Vec methods
impl<T> std::ops::DerefMut for NonEmptyVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Implementing From for creating NonEmptyVec from a single element
impl<T> From<T> for NonEmptyVec<T> {
    fn from(item: T) -> Self {
        NonEmptyVec(vec![item])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::hex::ToHexExt;
    use tokio::{sync::Mutex, time::Instant};

    #[derive(Debug, PartialEq, Deserialize)]
    struct TestStruct {
        #[serde(deserialize_with = "deserialize_csv_field")]
        field: Option<HashSet<String>>,
    }

    #[test]
    fn test_deserialize_csv_field_strings() {
        let input = r#"{ "field": "not-initiated,in-progress,completed,in-progress" }"#;
        let result: TestStruct = serde_json::from_str(input).unwrap();
        let expected_result = Some(HashSet::from([
            "not-initiated".to_string(),
            "in-progress".to_string(),
            "completed".to_string(),
        ]));

        assert_eq!(result.field, expected_result);
    }

    #[test]
    fn test_deserialize_csv_field_numbers() {
        let input = r#"{ "field": "1,2,3" }"#;
        let result: TestStruct = serde_json::from_str(input).unwrap();
        let expected_result = Some(HashSet::from([
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
        ]));

        assert_eq!(result.field, expected_result);
    }

    #[test]
    fn test_to_bytes_with_0x_prefix() {
        // Test conversion of hex string with 0x prefix
        let hex = "0x1a2b3c".to_string();
        let bytes = hex.hex_to_bytes().unwrap();

        // Expected result is [0x1a, 0x2b, 0x3c]
        let expected = Bytes::from(vec![0x1a, 0x2b, 0x3c]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_to_bytes_without_prefix() {
        // Test conversion of hex string without 0x prefix
        let hex = "1a2b3c".to_string();
        let bytes = hex.hex_to_bytes().unwrap();

        // Expected result is [0x1a, 0x2b, 0x3c]
        let expected = Bytes::from(vec![0x1a, 0x2b, 0x3c]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_to_bytes_string_slice() {
        // Test the implementation for &str
        let hex = "0x1a2b3c";
        let bytes = hex.hex_to_bytes().unwrap();

        // Expected result is [0x1a, 0x2b, 0x3c]
        let expected = Bytes::from(vec![0x1a, 0x2b, 0x3c]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_to_bytes_empty_string() {
        // Test with empty string
        let hex = "";
        let bytes = hex.hex_to_bytes().unwrap();

        // Expected result is empty bytes
        let expected = Bytes::new();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_to_bytes_empty_string_with_prefix() {
        // Test with just the prefix
        let hex = "0x";
        let bytes = hex.hex_to_bytes().unwrap();

        // Expected result is empty bytes
        let expected = Bytes::new();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_to_bytes_invalid_hex() {
        // Test with invalid hex characters
        let hex = "0x1g2b3c"; // 'g' is not a valid hex character
        let result = hex.hex_to_bytes();

        // Should return an error
        assert!(result.is_err());
    }

    #[test]
    fn test_to_bytes_roundtrip() {
        // Test round-trip conversion: bytes -> hex -> bytes
        let original_bytes = Bytes::from(vec![0x1a, 0x2b, 0x3c]);
        let hex = format!("0x{}", original_bytes.encode_hex());
        let round_trip_bytes = hex.hex_to_bytes().unwrap();

        // Round-trip should preserve the original bytes
        assert_eq!(original_bytes, round_trip_bytes);
    }

    #[test]
    fn test_hashable_string() {
        // Test the Hashable implementation for String
        let input = "hello world".to_string();
        let hash = input.sha256().unwrap();

        // Expected hash for "hello world"
        // echo -n "hello world" | shasum -a 256
        let expected_hex = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let expected_bytes = expected_hex.hex_to_bytes().unwrap();
        let expected = FixedBytes::<32>::from_slice(&expected_bytes);

        assert_eq!(hash, expected);
    }

    #[test]
    fn test_env_var_deserializer() {
        // Basic test for the env var deserializer
        // Set a test environment variable
        std::env::set_var("TEST_API_KEY", "secret_value");

        // Create a mock deserializer that will provide our test string
        struct MockDeserializer;

        impl<'de> Deserializer<'de> for MockDeserializer {
            type Error = serde::de::value::Error;

            fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
            where
                V: de::Visitor<'de>,
            {
                unimplemented!()
            }

            // Implement the method that deserialize_str calls
            fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
            where
                V: de::Visitor<'de>,
            {
                visitor.visit_str("#ENV:TEST_API_KEY")
            }

            // Forward other methods to deserialize_any
            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char string
                bytes byte_buf option unit unit_struct newtype_struct seq tuple
                tuple_struct map struct enum identifier ignored_any
            }
        }

        // Use our function to deserialize it
        let result = deserialize_env_field(MockDeserializer).unwrap();

        // Should get the env var value
        assert_eq!(result, "secret_value");

        // Clean up
        std::env::remove_var("TEST_API_KEY");
    }

    #[test]
    fn test_encrypted_env_var_deserializer() {
        // Basic test for the env var deserializer
        // Set a test environment variable
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cipher = Cipher::from_key(&key).unwrap();
        let plaintext = "secret_value";
        let encrypted = cipher.encrypt(plaintext.as_bytes()).unwrap();
        let enc_hex = hex::encode(encrypted);

        std::env::set_var("AES_SECRET_KEY", key);
        std::env::set_var("TEST_API_KEY", enc_hex);

        // Create a mock deserializer that will provide our test string
        struct MockDeserializer;

        impl<'de> Deserializer<'de> for MockDeserializer {
            type Error = serde::de::value::Error;

            fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
            where
                V: de::Visitor<'de>,
            {
                unimplemented!()
            }

            // Implement the method that deserialize_str calls
            fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
            where
                V: de::Visitor<'de>,
            {
                visitor.visit_str("#EncryptedENV:TEST_API_KEY")
            }

            // Forward other methods to deserialize_any
            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char string
                bytes byte_buf option unit unit_struct newtype_struct seq tuple
                tuple_struct map struct enum identifier ignored_any
            }
        }

        // Use our function to deserialize it
        let result = deserialize_env_field(MockDeserializer).unwrap();

        // Should get the env var value
        assert_eq!(result, "secret_value");

        // Clean up
        std::env::remove_var("TEST_API_KEY");
    }

    #[test]
    fn test_gen_secret() {
        let (secret, hash) = gen_secret();

        assert_eq!(secret.len(), 32);
        assert_eq!(hash.len(), 32);

        // Same secret should produce the same hash
        let hash2 = FixedBytes::new(sha2::Sha256::digest(secret.as_ref()).into());
        assert_eq!(hash, hash2);

        let (secret2, hash2) = gen_secret();

        // Different calls to gen_secret should produce different results
        assert_ne!(secret, secret2);
        assert_ne!(hash, hash2);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_success_scenario() {
        // Test successful retry with exponential backoff timing
        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();
        let start_time = Instant::now();

        let operation = move || {
            let count = call_count_clone.clone();
            async move {
                let mut num = count.lock().await;
                *num += 1;
                if *num < 3 {
                    Err("Temporary failure")
                } else {
                    Ok::<i32, &str>(42)
                }
            }
        };

        let result = retry_with_backoff(operation, 5, 50).await;
        let elapsed = start_time.elapsed();

        // Verify success
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(*call_count.lock().await, 3);

        // Verify exponential backoff timing: 50ms + 100ms = ~150ms
        assert!(elapsed.as_millis() >= 150);
        assert!(elapsed.as_millis() < 200); // Allow overhead
    }

    #[tokio::test]
    async fn test_retry_with_backoff_failure_and_edge_cases() {
        // Test failure scenario with custom error type and edge case max_retries=1
        #[derive(Debug, PartialEq)]
        struct CustomError(String);

        impl std::fmt::Display for CustomError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        let operation = move || {
            let count = call_count_clone.clone();
            async move {
                let mut num = count.lock().await;
                *num += 1;
                Err::<String, CustomError>(CustomError("Persistent failure".to_string()))
            }
        };

        // Test with single retry (edge case)
        let result = retry_with_backoff(operation, 1, 10).await;

        // Verify failure handling
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().0, "Persistent failure");
        assert_eq!(*call_count.lock().await, 1); // Only one attempt
    }

    #[cfg(test)]
    mod non_empty_vec_tests {
        use super::*;

        /// Tests successful creation of NonEmptyVec
        #[test]
        fn test_new_success() {
            let vec = vec![1, 2, 3];
            let result = NonEmptyVec::new(vec.clone());
            assert!(result.is_ok());
            let non_empty = result.unwrap();
            assert_eq!(non_empty.as_ref(), &vec);
        }

        /// Tests creation failure with empty vector
        #[test]
        fn test_new_empty_fails() {
            let vec: Vec<i32> = vec![];
            let result = NonEmptyVec::new(vec);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "expected a non-empty vector"
            );
        }

        /// Tests TryFrom implementation
        #[test]
        fn test_try_from() {
            let vec = vec!["test".to_string()];
            let result: eyre::Result<NonEmptyVec<String>> = NonEmptyVec::try_from(vec.clone());
            assert!(result.is_ok());
            assert_eq!(result.unwrap().as_ref(), &vec);
        }

        /// Tests TryFrom with empty vector
        #[test]
        fn test_try_from_empty() {
            let vec: Vec<String> = vec![];
            let result: eyre::Result<NonEmptyVec<String>> = NonEmptyVec::try_from(vec.clone());
            assert!(result.is_err());
        }

        /// Tests first method
        #[test]
        fn test_first() {
            let non_empty = NonEmptyVec::new(vec![1, 2, 3]).unwrap();
            assert_eq!(non_empty.first(), &1);
        }

        /// Tests length method
        #[test]
        fn test_len() {
            let non_empty = NonEmptyVec::new(vec![1, 2, 3]).unwrap();
            assert_eq!(non_empty.len(), 3);
        }

        /// Tests From implementation for single element
        #[test]
        fn test_from_single() {
            let non_empty: NonEmptyVec<i32> = 42.into();
            assert_eq!(non_empty.len(), 1);
            assert_eq!(non_empty.first(), &42);
        }

        /// Tests Deref implementation
        #[test]
        fn test_deref() {
            let non_empty = NonEmptyVec::new(vec![1, 2, 3]).unwrap();
            assert_eq!(non_empty[0], 1);
            assert_eq!(non_empty.get(1), Some(&2));
        }

        /// Tests DerefMut implementation
        #[test]
        fn test_deref_mut() {
            let mut non_empty = NonEmptyVec::new(vec![1, 2, 3]).unwrap();
            non_empty[0] = 10;
            assert_eq!(non_empty.first(), &10);
        }
    }
}
