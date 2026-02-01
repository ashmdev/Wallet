use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub webhook: WebhookConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_grpc_max_message_size")]
    pub grpc_max_message_size: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_acquire_timeout")]
    pub acquire_timeout_secs: u64,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_max_lifetime")]
    pub max_lifetime_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebhookConfig {
    #[serde(default = "default_webhook_enabled")]
    pub enabled: bool,
    #[serde(default = "default_webhook_queue_size")]
    pub queue_size: usize,
    #[serde(default = "default_webhook_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_webhook_max_retries")]
    pub max_retries: u32,
    pub endpoint_url: Option<String>,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    50051
}

fn default_grpc_max_message_size() -> usize {
    4 * 1024 * 1024 // 4MB
}

fn default_max_connections() -> u32 {
    100
}

fn default_min_connections() -> u32 {
    10
}

fn default_acquire_timeout() -> u64 {
    30
}

fn default_idle_timeout() -> u64 {
    600
}

fn default_max_lifetime() -> u64 {
    1800
}

fn default_webhook_enabled() -> bool {
    false
}

fn default_webhook_queue_size() -> usize {
    10000
}

fn default_webhook_timeout() -> u64 {
    30
}

fn default_webhook_max_retries() -> u32 {
    5
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            grpc_max_message_size: default_grpc_max_message_size(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://localhost/veloxledger".to_string(),
            max_connections: default_max_connections(),
            min_connections: default_min_connections(),
            acquire_timeout_secs: default_acquire_timeout(),
            idle_timeout_secs: default_idle_timeout(),
            max_lifetime_secs: default_max_lifetime(),
        }
    }
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: default_webhook_enabled(),
            queue_size: default_webhook_queue_size(),
            timeout_secs: default_webhook_timeout(),
            max_retries: default_webhook_max_retries(),
            endpoint_url: None,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, config::ConfigError> {
        let config = config::Config::builder()
            .add_source(
                config::Environment::with_prefix("VELOX")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?;

        config.try_deserialize()
    }

    pub fn from_file(path: &str) -> Result<Self, config::ConfigError> {
        let config = config::Config::builder()
            .add_source(config::File::with_name(path))
            .add_source(
                config::Environment::with_prefix("VELOX")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?;

        config.try_deserialize()
    }
}
