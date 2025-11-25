use std::sync::Arc;
use std::env;
use dotenv::dotenv;
use serde::{Deserialize, Serialize}; 
use sqlx::PgPool;
use crate::DbRepository; 
use eyre::Result;
use reqwest::Client;
use bitcoin::BlockHeader;
use bitcoin::consensus::deserialize as bitcoin_deserialize;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MerkleProof {
    pub block_height: u32,
    pub merkle: Vec<String>,
    pub pos: usize,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TX {
    pub txid: String,
    pub version: i32,
    pub locktime: i64,
    pub vin: Vec<TVin>,
    pub vout: Vec<TVout>,
    pub status: Status,
    pub fee: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TVin {
    pub txid: String,
    pub vout: u32,
    pub prevout: Option<TVout>,
    pub scriptsig: String,
    pub sequence: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TVout {
    pub scriptpubkey: String,
    pub scriptpubkey_address: Option<String>,
    pub value: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Status {
    pub confirmed: bool,
    pub block_height: Option<i32>,
    pub block_hash: Option<String>,
    pub block_time: Option<i64>,
}

pub struct BlockstreamClient {
    base_url: String,
    client: Client,
}

impl BlockstreamClient {
    pub fn new(network: bool) -> Self {
        let base_url = if network {
            "https://blockstream.info/api".to_string()
        } else {
            "https://blockstream.info/testnet/api".to_string()
        };
        Self {
            base_url,
            client: Client::new(),
        }
    }

    pub async fn get_tip_height(&self) -> Result<u32> {
        let url = format!("{}/blocks/tip/height", self.base_url);
        let response = self.client.get(&url).send().await?;
        let text = response.text().await?;
        let height: u32 = text.parse()?;
        Ok(height)
    }

    pub async fn get_block_hash(&self, height: u32) -> Result<String> {
        let url = format!("{}/block-height/{}", self.base_url, height);
        let response = self.client.get(&url).send().await?;
        let hash = response.text().await?;
        Ok(hash)
    }

    pub async fn get_block_header(&self, block_hash: &str) -> Result<BlockHeader> {
        let url = format!("{}/block/{}/header", self.base_url, block_hash);
        let hex_string = self.client.get(&url).send().await?.text().await?;
        let bytes = hex::decode(hex_string)?;
        let header: BlockHeader = bitcoin_deserialize(&bytes)?;
        Ok(header)
    }

    pub async fn get_address_txs(&self, address: &str) -> Result<Vec<TX>> {
        let url = format!("{}/address/{}/txs", self.base_url, address);
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
             return Err(eyre::eyre!("API Error: {:?}", response.status()));
        }
        let txs = response.json::<Vec<TX>>().await?;
        Ok(txs)
    }

    pub async fn get_merkle_proof(&self, txid: &str) -> Result<MerkleProof> {
        let url = format!("{}/tx/{}/merkle-proof", self.base_url, txid);
        let proof = self.client.get(&url).send().await?.json::<MerkleProof>().await?;
        Ok(proof)
    }
}

#[derive(Clone)]
pub struct PgRepository {
    pub pool: PgPool,
}

impl PgRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String, 
    pub exp: i64,  
}

#[derive(Clone)]
pub struct AppState {
    pub db_repo: Arc<dyn DbRepository>,
    pub config: Arc<Config>, 
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub port: u16,
    pub ip_musk: Option<String>,
    pub network: bool,
    pub reorg_limit: u16,
    
}

impl Config {
    pub fn load() -> Result<Self> {
        dotenv().ok();
        let database_url = env::var("DATABASE_URL")?;
        let jwt_secret = env::var("JWT_STRING")?;
        let port: u16  = env::var("API_PORT")?.parse()?;
        let ip_musk = env::var("IP_MASK").ok();
        let network: bool = match env::var("MAINNET") {
            Ok(val) => val.parse().unwrap_or(true),
            Err(_) => true,
        };
        let reorg_limit: u16= env::var("MAX_REORG_CAPACITY")?.parse()?;

        Ok(Config {
            database_url,
            jwt_secret,
            port,   
            ip_musk,  
            network,
            reorg_limit,
        })
    }
}