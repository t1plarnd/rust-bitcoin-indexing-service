use async_trait::async_trait;
use eyre::Result;
use sqlx::{Error as SqlxError};
use sqlx::Row;
use tokio::task::spawn_blocking;
use bcrypt::{hash, verify};
use bitcoin::BlockHash;
use std::str::FromStr;
pub mod models;
pub mod responses;
use models::{PgRepository, InfoResponse, UtxoPayload};
use responses::{
    TrackedAddress, 
    AddressPayload, 
    RegisterData,
    AddressUtxoResponse,
    BalanceResponse};

#[async_trait]
pub trait DbRepository: Send + Sync {
    async fn create_user(&self, data: &RegisterData) -> Result<bool, eyre::Error>;
    async fn check_user(&self, data: &RegisterData) -> Result<bool, eyre::Error>;
    async fn add_tracked_addresses(&self, user_id: i32, addresses: Vec<String>) -> Result<bool, eyre::Error>;
    async fn get_addresses_for_user(&self, user_id: i32) -> Result<Vec<String>, eyre::Error>;
    async fn get_all_tracked_addresses(&self) -> Result<Vec<TrackedAddress>, eyre::Error>;
    async fn get_user_id_by_username(&self, username: &str) -> Result<i32, eyre::Error>;
    async fn get_utxos_for_user_address(&self, addresses: Vec<String>) -> Result<Vec<AddressUtxoResponse>, eyre::Error>;
    async fn get_balance_for_user_addresses(&self, user_id: i32, addresses: Vec<String>) -> Result<Vec<BalanceResponse>, eyre::Error>;
    async fn get_utxo_history_for_user_addresses(&self, address: Vec<String>) -> Result<Vec<AddressUtxoResponse>, eyre::Error>;
    async fn mark_utxo_spent(&self, txid: &str, vout_idx: i32) -> Result<(), eyre::Error>;
    async fn check_user_address_access(&self, user_id: i32, payload: AddressPayload) -> Result<bool, eyre::Error>;
    async fn delete_utxos(&self, txids: Vec<TrackedAddress>, height: i32) -> Result<(), eyre::Error>;
    async fn save_utxos(&self, utxos: Vec<UtxoPayload>) -> Result<(), eyre::Error>;
    async fn save_info(&self, height:u32, hash:String)->Result<(), eyre::Error>;
    async fn get_info(&self, h: u32, has: BlockHash)->Result<InfoResponse, eyre::Error>;
}

#[async_trait]
impl DbRepository for PgRepository {
    async fn create_user(&self, data: &RegisterData) -> Result<bool, eyre::Error> {
        let password_to_hash = data.password.clone();
        let hashed = spawn_blocking(move || hash(password_to_hash, 10)).await??;
        let result = sqlx::query(
            "INSERT INTO users (username, hashed_password) 
             VALUES ($1, $2)
             ON CONFLICT (username) DO NOTHING"
        )
        .bind(&data.username)
        .bind(&hashed)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
    async fn check_user(&self, data: &RegisterData) -> Result<bool, eyre::Error> {
        let record = match sqlx::query(
            "SELECT hashed_password 
             FROM users 
             WHERE username = $1"
        )
        .bind(&data.username)
        .fetch_one(&self.pool)
        .await {
            Ok(record) => record,
            Err(SqlxError::RowNotFound) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let provided_password = data.password.clone();
        let stored_hash: String = record.get("hashed_password");
        let is_valid = spawn_blocking(move || verify(provided_password, &stored_hash)).await??;
        Ok(is_valid)
    }
    async fn add_tracked_addresses(&self, user_id: i32, addresses: Vec<String>) -> Result<bool, eyre::Error> {
        let mut tx = self.pool.begin().await?;
        for address in addresses {
            let query_result = sqlx::query(
                "INSERT INTO tracked_addresses (user_id, address) 
                 VALUES ($1, $2)
                 ON CONFLICT (user_id, address) DO NOTHING"
            )
            .bind(user_id)
            .bind(address)
            .execute(&mut *tx) 
            .await;

            if let Err(_e) = query_result {
                tx.rollback().await?; 
                return Ok(false); 
            }
        }
        tx.commit().await?;
        Ok(true)
    }
    async fn get_addresses_for_user(&self, user_id: i32) -> Result<Vec<String>, eyre::Error> {
        let addresses = sqlx::query(
            "SELECT address 
             FROM tracked_addresses
             WHERE user_id = $1"
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row|{
                row.get::<String, _>("address")
            }).collect();
        Ok(addresses)
    }
    async fn get_all_tracked_addresses(&self) -> Result<Vec<TrackedAddress>, eyre::Error> {
        let addresses = sqlx::query(
            "SELECT address_id, address
             FROM tracked_addresses"
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| TrackedAddress {
            address_id: row.get("address_id"),
            address: row.get("address"),
        })
        .collect();
        Ok(addresses)
    }
    async fn get_user_id_by_username(&self, username: &str) -> Result<i32, eyre::Error> {
        let user_id = sqlx::query(
            "SELECT user_id
             FROM users 
             WHERE username = $1"
        )
        .bind(username)
        .fetch_one(&self.pool)
        .await?
        .get("user_id");

        Ok(user_id)
    }
    async fn get_utxos_for_user_address(&self, addresses: Vec<String>) -> Result<Vec<AddressUtxoResponse>, eyre::Error> {
        let utxos = sqlx::query(
            r#"
            SELECT 
                u.address, 
                u.vout_idx,
                u.txid,
                u.block_hash,
                u.vouts,
                u.vins,
                u.value, 
                u.block_height
            FROM utxos u
            INNER JOIN tracked_addresses ta ON u.address_id = ta.address_id
            WHERE ta.address = ANY($1) 
                AND u.is_spent = FALSE   
            ORDER BY u.block_height DESC NULLS FIRST, u.txid
            "#
        )
        .bind(&addresses) 
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
                AddressUtxoResponse {
                address: row.get("address"),     
                txid: row.get("txid"), 
                vout_idx: row.get("vout_idx"), 
                block_hash: row.get("block_hash"), 
                vouts: row.get("vins"), 
                vins:  row.get("vouts"),         
                value: row.get("value"),               
                block_height: row.get("block_height"), 
                is_spent: false,
            }}).collect();
        Ok(utxos)
    }
    async fn get_balance_for_user_addresses(&self, user_id: i32, addresses: Vec<String>) -> Result<Vec<BalanceResponse>, eyre::Error> {
        let balances = sqlx::query(
            r#"
            SELECT ta.address, COALESCE(SUM(u.value), 0)::BIGINT as balance
            FROM tracked_addresses ta
            LEFT JOIN utxos u ON ta.address_id = u.address_id AND u.is_spent = FALSE
            WHERE ta.user_id = $1 
                AND ta.address = ANY($2)
            GROUP BY ta.address
            "#
        )
        .bind(user_id)
        .bind(&addresses)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            let balance_i64: i64 = row.get("balance");
            BalanceResponse {
                address: row.get("address"),
                balance: balance_i64 as u64, 
            }}).collect();
        Ok(balances)
    }
    async fn get_utxo_history_for_user_addresses(&self, addresses: Vec<String>) -> Result<Vec<AddressUtxoResponse>, eyre::Error> {
        let history= sqlx::query(
                r#"SELECT u.address, 
                          u.txid, 
                          u.vout_idx,
                          u.block_hash, 
                          u.vouts, 
                          u.vins, 
                          u.value, 
                          u.block_height, 
                          u.is_spent 
                 FROM utxos u  
                 WHERE u.address = ANY($1) 
                 ORDER BY u.block_height DESC NULLS FIRST, u.txid"#
            )
            .bind(&addresses)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                AddressUtxoResponse {
               address: row.get("address"),     
                txid: row.get("txid"), 
                vout_idx: row.get("vout_idx"), 
                block_hash: row.get("block_hash"), 
                vouts: row.get("vins"), 
                vins:  row.get("vouts"),         
                value: row.get("value"),               
                block_height: row.get("block_height"), 
                is_spent: row.get("is_spent"),
                }}).collect();
        Ok(history)
    }
    async fn delete_utxos(&self, txids: Vec<TrackedAddress>, height: i32) -> Result<(), eyre::Error> {
        if txids.is_empty() {
            return Ok(());
        }
        let list = txids.into_iter().map(|t| t.address).collect::<Vec<String>>();   
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE 
                     FROM utxos 
                     WHERE txid = ANY($1) 
                        AND block_height >= $2")
            .bind(list)
            .bind(height)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn check_user_address_access(&self, user_id: i32, payload: AddressPayload) -> Result<bool, eyre::Error> {
        let counter = payload.addresses.len() as i64;
        let db_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) as addresses_owned
             FROM tracked_addresses 
             WHERE user_id = $1        
                 AND address = ANY($2)"
        )
        .bind(user_id)
        .bind(&payload.addresses)
        .fetch_one(&self.pool)
        .await?;
        
        if counter == db_count {
            Ok(true)
        }
        else{
            Ok(false)
        }
    }
    async fn save_utxos(&self, items: Vec<UtxoPayload>) -> Result<(), eyre::Error> {
        let mut tx = self.pool.begin().await?;

        for item in items {
            sqlx::query(
                "INSERT INTO utxos (
                    address_id, txid, vout_idx, block_hash, block_time, 
                    vouts, vins, value, block_height, is_spent
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (txid, vout_idx) DO NOTHING"
            )
            .bind(item.address_id)
            .bind(&item.txid)
            .bind(item.vout_idx) 
            .bind(&item.block_hash)
            .bind(item.block_time)
            .bind(sqlx::types::Json(&item.vouts))
            .bind(sqlx::types::Json(&item.vins))
            .bind(item.value)
            .bind(item.block_height)
            .bind(false)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
    async fn mark_utxo_spent(&self, txid: &str, vout_idx: i32) -> Result<(), eyre::Error> {
        sqlx::query(
            "UPDATE utxos 
             SET is_spent = TRUE 
             WHERE txid = $1 AND vout_idx = $2"
        )
        .bind(txid)
        .bind(vout_idx)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    async fn save_info(&self, height: u32, hash: String) -> Result<(), eyre::Error> {
        sqlx::query("INSERT INTO info (height, block_hash) VALUES ($1, $2) ON CONFLICT (block_hash) DO NOTHING")
            .bind(height as i32) 
            .bind(hash)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
    async fn get_info(&self, h: u32, has: BlockHash) -> Result<InfoResponse, eyre::Error> {
        let row_opt = sqlx::query("SELECT height, block_hash FROM info ORDER BY height DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        match row_opt {
            Some(row) => {
                let height: i32 = row.try_get("height")?;
                let hash: String = row.try_get("block_hash")?;
                Ok(InfoResponse {
                    height: height as u32, 
                    hash_rpc: hash.clone(),
                    hash_p2p: BlockHash::from_str(&hash)?
                })
            },
            None => {
                Ok(InfoResponse {
                    height: h,
                    hash_rpc: has.to_string(),
                    hash_p2p: has,
                })
            }
        }
    }
}