use eyre::Result;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use api::run;
use indexer::run_indexer;
use handling::{AppState, Config};
use db::{DbRepository, PgRepository};
use nakamoto_client::Network;
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
    let network = Network::Mainnet;
    let indexer_repo = db_repo.clone(); 
    let path = app_config.nakamoto_path.clone();

    tokio::spawn(async move {
       let _ = run_indexer(indexer_repo, network, path).await;
    });

    let app_state = AppState { 
        db_repo, 
        config: Arc::new(app_config.clone())
    };
    run(app_state).await?;

    Ok(())
}