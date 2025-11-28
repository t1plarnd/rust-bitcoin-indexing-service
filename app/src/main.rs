use eyre::Result;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tracing::{info, error, warn};
use tokio::time::{sleep, Duration};
use api::run;
use db::DbRepository;
use db::models::{AppState, Config, PgRepository};
use p2p_indexer::run_p2p_indexer;
use rpc_indexer::run_rpc_indexer;

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
            std::process::exit(1);
        }
    }
    
    let db_repo_impl = PgRepository::new(pool.clone());
    let db_repo: Arc<dyn DbRepository> = Arc::new(db_repo_impl);

    let app_state_rpc_indx = AppState { 
        db_repo: db_repo.clone(), 
        config: Arc::new(app_config.clone())
    };
    
    let app_state_p2p_indx = AppState { 
        db_repo: db_repo.clone(), 
        config: Arc::new(app_config.clone())
    };

    let app_state_api = AppState { 
        db_repo, 
        config: Arc::new(app_config.clone())
    };
    
    info!("Starting Resilient P2P Indexer...");
    spawn_resilient_p2p_indexer(app_state_p2p_indx);

    info!("Starting Resilient RPC Indexer...");
    spawn_resilient_rpc_indexer(app_state_rpc_indx);

    info!("Api running");
    run(app_state_api).await?;

    Ok(())
}
fn spawn_resilient_p2p_indexer(state: AppState) {
    tokio::spawn(async move {
        loop {
            let state_clone = state.clone();
            info!("Spawning P2P worker task...");

            let handle = tokio::spawn(async move {
                run_p2p_indexer(state_clone).await
            });

            match handle.await {
                Ok(Ok(_)) => warn!("P2P Indexer finished normally. Restarting in 5s..."),
                Ok(Err(e)) => error!("P2P Indexer failed: {}. Restarting in 5s...", e),
                Err(join_err) => {
                    if join_err.is_panic() {
                        error!("P2P INDEXER PANICKED! Restarting in 5s...");
                    } else {
                        error!("P2P Task cancelled. Restarting...");
                    }
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    });
}
fn spawn_resilient_rpc_indexer(state: AppState) {
    tokio::spawn(async move {
        loop {
            let state_clone = state.clone();
            info!("Spawning RPC worker task...");

            let handle = tokio::spawn(async move {
                run_rpc_indexer(state_clone).await
            });

            match handle.await {
                Ok(Ok(_)) => warn!("RPC Indexer finished normally. Restarting in 5s..."),
                Ok(Err(e)) => error!("RPC Indexer failed: {}. Restarting in 5s...", e),
                Err(join_err) => {
                    if join_err.is_panic() {
                        error!("RPC INDEXER PANICKED! Restarting in 5s...");
                    } else {
                        error!("RPC Task cancelled. Restarting...");
                    }
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    });
}