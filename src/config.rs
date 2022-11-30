use config::{Config, ConfigError, Environment, File};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Settings {
    pub ipfs_gateways: Vec<String>,
    pub ipfs_cache_directory: String,
    pub user_agent: String,
    pub connect_timeout: u64,
    pub max_retries: u32,
    pub pause_gateway_seconds: i64,
    pub delete_after_days: usize,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let env_override = Environment::default().separator("__");
        let run_mode = std::env::var("ENV").unwrap_or_else(|_| "development".into());

        let settings = Config::builder()
            .add_source(File::with_name("config/config"))
            .add_source(File::with_name("config/local").required(false))
            .add_source(File::with_name(&format!("config/{}", run_mode)).required(false))
            .add_source(env_override)
            .build()?;

        settings.try_deserialize()
    }
}
