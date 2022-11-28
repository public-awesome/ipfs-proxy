use sea_orm::{Database, DatabaseConnection};
use std::fs::File;
use std::path::Path;

use crate::config::Settings;

pub struct AppContext {
    pub db: DatabaseConnection,
    pub config: Settings,
}

impl AppContext {
    pub async fn build() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            let filename = "objects.sqlite";
            if !Path::new(filename).exists() {
                File::create(filename).expect("Can't create DB");
            }
            "sqlite://objects.sqlite".to_string()
        });
        let db = match Database::connect(database_url).await {
            Err(err) => {
                panic!("Could not connect to database: {err}");
            }
            Ok(db) => db,
        };
        let config = Settings::new().expect("Can't create configuration");
        AppContext { db, config }
    }
}
