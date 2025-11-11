use eyre::Result;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use models::Config;
use api::run;
use indexer::run_indexer;
use api::AppState;
use db::{DbRepository, PgRepository};
use nakamoto_client::Network;

#[tokio::main]
async fn main() -> Result<()> {
    let app_config = Config::load()?;
    println!("Config loaded.");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&app_config.database_url)
        .await?;
    println!("Database connected.");
    match sqlx::migrate!("../crates/db/migrations").run(&pool).await {
        Ok(_) => println!("Migrations applied successfully."),
        Err(e) => {
            eprintln!("Migration error: {}", e);
            eprintln!("Try running: docker-compose down --volumes && docker-compose up");
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