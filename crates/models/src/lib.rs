use serde::{Deserialize, Serialize};
use dotenv::dotenv;
use eyre::Result;
use std::{env, path::PathBuf};
use sqlx::FromRow;


#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub nakamoto_path: PathBuf,
    
}
impl Config {
    pub fn load() -> Result<Self> {
        dotenv().ok();
        let database_url = env::var("DATABASE_URL")?;
        let jwt_secret = env::var("JWT_STRING")?;
        let nakamoto_path = env::var("NAKAMOTO_PATH")?;
        Ok(Config {
            database_url,
            jwt_secret,
            nakamoto_path: nakamoto_path.into(),
        })
    }
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String, 
    pub exp: i64,  
}
#[derive(Debug, Deserialize)]
pub struct RegisterData{
    pub username: String,
    pub password: String, 
}
#[derive(Debug, Deserialize)]
pub struct UtxoData {
    pub txid: String,
    pub vout: i32,
    pub value: i64,
}
#[derive(Debug)]
pub struct NewTransaction {
    pub txid: String,
    pub block_height: Option<i32>,
    pub block_hash: Option<String>,
    pub block_time: Option<chrono::NaiveDateTime>,
}
#[derive(Debug)]
pub struct NewTxOutput {
    pub address_id: i32,
    pub txid: String,
    pub vout: i32,
    pub value: i64,
}
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct UtxoResponse {
    pub txid: String,
    pub vout: i32,
    pub value: i64,
    pub block_height: Option<i32>, 
}
