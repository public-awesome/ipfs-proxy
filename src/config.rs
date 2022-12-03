use config::{Config, ConfigError, Environment, File};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Settings {
    pub ipfs_gateways: Vec<String>,
    pub ipfs_cache_directory: String,
    pub user_agent: String,
    pub connect_timeout: u64,
    pub pause_gateway_seconds: i64,
    pub delete_after_days: i64,
    pub max_content_length: u64,
    pub server_port: u16,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let env_override = Environment::default().separator("__");
        let run_mode = if cfg!(test) {
            "test".to_string()
        } else {
            std::env::var("ENV").unwrap_or_else(|_| "development".to_string())
        };

        let settings = Config::builder()
            .add_source(File::with_name("config/config"))
            .add_source(File::with_name(&format!("config/{}", run_mode)).required(false))
            .add_source(File::with_name("config/local").required(false))
            .add_source(env_override)
            .build()?;

        settings.try_deserialize()
    }
}
