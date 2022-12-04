use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use std::fs::File;
use std::path::Path;

use crate::config::Settings;

pub struct AppContext {
    pub db: DatabaseConnection,
    pub config: Settings,
}

impl AppContext {
    pub async fn build() -> Self {
        let config = Settings::new().expect("Can't create configuration");

        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            let filename = "objects.sqlite";
            if !Path::new(filename).exists() {
                File::create(filename).expect("Can't create DB");
            }
            "sqlite://objects.sqlite".to_string()
        });

        let mut opt = ConnectOptions::new(database_url);
        opt.max_connections(config.db_max_connections)
            .min_connections(config.db_min_connections);

        let db = match Database::connect(opt).await {
            Err(err) => {
                panic!("Could not connect to database: {err}");
            }
            Ok(db) => db,
        };

        // For faster execution using multithread
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "PRAGMA journal_mode=WAL;".to_owned(),
        ))
        .await
        .expect("Can't set PRAGMA");

        AppContext { db, config }
    }
}
