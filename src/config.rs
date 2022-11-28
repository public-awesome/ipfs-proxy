use config::{Config, ConfigError, Environment, File};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Settings {
    pub ipfs_gateways: Vec<String>,
    pub ipfs_cache_directory: String,
    pub user_agent: String,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let env_override = Environment::default().separator("__");

        let settings = Config::builder()
            .add_source(File::with_name("config"))
            .add_source(env_override)
            .build()?;

        settings.try_deserialize()
    }
}
