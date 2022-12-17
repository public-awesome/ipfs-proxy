use config::{Config, ConfigError, Environment, File};

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Settings {
    pub ipfs_gateways: Vec<String>,
    pub ipfs_cache_directory: String,
    pub user_agent: String,
    pub connect_timeout: u64,
    pub pause_gateway_seconds: i64,
    pub delete_after_days: i64,
    pub max_content_length: u64,
    pub server_port: u16,
    pub db_max_connections: u32,
    pub db_min_connections: u32,
    pub permitted_resize_dimensions: Vec<Dimension>,
    pub ipfs: IpfsConfig,
}

#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Dimension {
    pub width: u32,
    pub height: u32,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct IpfsConfig {
    pub enabled: bool,
    pub binary_path: String,
}

impl Settings {
    pub fn full_ipfs_cache_directory(&self) -> String {
        if self.ipfs_cache_directory.starts_with('/') {
            self.ipfs_cache_directory.clone()
        } else {
            format!(
                "{}/{}",
                std::env::current_dir()
                    .expect("Can't get current directory")
                    .display(),
                self.ipfs_cache_directory
            )
        }
    }

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
