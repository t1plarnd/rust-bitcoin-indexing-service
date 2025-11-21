use eyre::Result;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use api::run;
use indexer::run_indexer;
use db::DbRepository;
use db::models::{AppState, Config, PgRepository};
use tracing::{info, error};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let app_config = Config::load()?;
    info!("Config loaded.");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&app_config.database_url)
        .await?;
    info!("Database connected.");
    match sqlx::migrate!("../crates/db/migrations").run(&pool).await {
        Ok(_) => info!("Migrations applied successfully."),
        Err(e) => {
            error!("Migration error: {}", e);
            error!("Try running: docker-compose down --volumes && docker-compose up");
            std::process::exit(1);
        }
    }
    let db_repo_impl = PgRepository::new(pool.clone());
    let db_repo: Arc<dyn DbRepository> = Arc::new(db_repo_impl);
    let app_state_indx = AppState { 
        db_repo: db_repo.clone(), 
        config: Arc::new(app_config.clone())
    };
    info!("Indexer running.");
    tokio::spawn(async move {
        if let Err(e) = indexer::run_indexer(app_state_indx).await {
            tracing::error!("Indexer crashed: {}", e);
        }
    });
    let app_state_api = AppState { 
        db_repo, 
        config: Arc::new(app_config.clone())
    };
    info!("Api running.");
    run(app_state_api).await?;

    Ok(())
}