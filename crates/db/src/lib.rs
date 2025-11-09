use async_trait::async_trait;
use eyre::Result;
use models::{RegisterData, NewTxOutput, NewTransaction, UtxoResponse};
use serde::{Deserialize, Serialize};
use sqlx::{Error as SqlxError, PgPool};
use tokio::task::spawn_blocking;
use bcrypt::{hash, verify};

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackedAddress {
    pub address_id: i32,
    pub address: String,
}


#[async_trait]
pub trait DbRepository: Send + Sync {
    async fn create_user(&self, data: &RegisterData) -> Result<bool, eyre::Error>;
    async fn check_user(&self, data: &RegisterData) -> Result<bool, eyre::Error>;
    async fn add_tracked_address(&self, user_id: i32, address: &str) -> Result<i32, eyre::Error>;
    async fn get_addresses_for_user(&self, user_id: i32) -> Result<Vec<TrackedAddress>, eyre::Error>;
    async fn get_all_tracked_addresses(&self) -> Result<Vec<TrackedAddress>, eyre::Error>;
    async fn get_user_id_by_username(&self, username: &str) -> Result<i32, eyre::Error>;
    async fn save_transaction(&self, tx: NewTransaction, outputs: &[NewTxOutput]) -> Result<(), eyre::Error>;
    async fn get_utxos_for_user_address(&self, user_id: i32, address: &str) -> Result<Vec<UtxoResponse>, eyre::Error>;
    async fn get_balance_for_user_address(&self, user_id: i32, address: &str) -> Result<i64, eyre::Error>;
    async fn get_transaction_history_for_user_address(&self, user_id: i32, address: &str) -> Result<Vec<UtxoResponse>, eyre::Error>;
    async fn mark_utxo_spent(&self, txid: &str, vout: i32) -> Result<(), eyre::Error>;
}

#[derive(Clone)]
pub struct PgRepository {
    pool: PgPool,
}

impl PgRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DbRepository for PgRepository {
    async fn create_user(&self, data: &RegisterData) -> Result<bool, eyre::Error> {
        let password_to_hash = data.password.clone();
        let hashed = spawn_blocking(move || {
            hash(password_to_hash, 10) 
        })
        .await??;

        let result = sqlx::query!(
            r#"INSERT INTO users (username, hashed_password) 
               VALUES ($1, $2)
               ON CONFLICT (username) DO NOTHING"#,
            data.username,
            hashed
        )
        .execute(&self.pool)
        .await?;
        
        Ok(result.rows_affected() > 0)
    }
    async fn check_user(&self, data: &RegisterData) -> Result<bool, eyre::Error> {
        let record = match sqlx::query!(
            r#"SELECT hashed_password FROM users WHERE username = $1"#,
            data.username
        )
        .fetch_one(&self.pool)
        .await
        {
            Ok(record) => record,
            Err(SqlxError::RowNotFound) => {
                return Ok(false);
            }
            Err(e) => {
                return Err(e.into());
            }
        };

        let provided_password = data.password.clone();
        let stored_hash = record.hashed_password;

        let is_valid = spawn_blocking(move || {
            verify(provided_password, &stored_hash)
        })
        .await??;
        Ok(is_valid)
    }
    async fn add_tracked_address(&self, user_id: i32, address: &str) -> Result<i32, eyre::Error> {
        let record = sqlx::query!(
            r#"INSERT INTO tracked_addresses (user_id, address) 
               VALUES ($1, $2)
               ON CONFLICT (user_id, address) DO UPDATE SET address = $2
               RETURNING address_id"#,
            user_id,
            address
        )
        .fetch_one(&self.pool)
        .await?;
        
        Ok(record.address_id)
    }
    async fn get_addresses_for_user(&self, user_id: i32) -> Result<Vec<TrackedAddress>, eyre::Error> {
        let addresses = sqlx::query_as!(
            TrackedAddress,
            "SELECT address_id, address FROM tracked_addresses WHERE user_id = $1",
            user_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(addresses)
    }
    async fn get_all_tracked_addresses(&self) -> Result<Vec<TrackedAddress>, eyre::Error> {
        let addresses = sqlx::query_as!(
            TrackedAddress,
            "SELECT address_id, address FROM tracked_addresses"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(addresses)
    }
    async fn get_user_id_by_username(&self, username: &str) -> Result<i32, eyre::Error> {
        let user_id = sqlx::query_scalar!(
            "SELECT user_id FROM users WHERE username = $1",
            username
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(user_id)
    }
    async fn save_transaction(&self, tx_data: NewTransaction, outputs: &[NewTxOutput]) -> Result<(), eyre::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            r#"INSERT INTO transactions (txid, block_height, block_hash, block_time)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (txid) DO NOTHING"#,
            tx_data.txid,
            tx_data.block_height,
            tx_data.block_hash,
            tx_data.block_time
        )
        .execute(&mut *tx)
        .await?;
        for output in outputs {
            sqlx::query!(
                r#"INSERT INTO utxos (address_id, txid, vout, value, status)
                   VALUES ($1, $2, $3, $4, 'unspent')
                   ON CONFLICT (txid, vout) DO NOTHING"#, 
                output.address_id,
                output.txid,
                output.vout,
                output.value
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
    async fn mark_utxo_spent(&self, txid: &str, vout: i32) -> Result<(), eyre::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            r#"UPDATE utxos SET status = 'spent' WHERE txid = $1 AND vout = $2 AND status = 'unspent'"#,
            txid,
            vout
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn get_utxos_for_user_address(&self, user_id: i32, address: &str) -> Result<Vec<UtxoResponse>, eyre::Error> {

        
        let utxos = sqlx::query_as!(
            UtxoResponse,
            r#"
            SELECT 
                o.txid, o.vout, o.value, t.block_height
            FROM utxos o  
            INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
            INNER JOIN transactions t ON o.txid = t.txid
            WHERE 
                ta.address = $1 
                AND ta.user_id = $2 
                AND o.status = 'unspent' 
            ORDER BY t.block_height DESC NULLS FIRST, o.txid
            "#,
            address,
            user_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(utxos)
    }
    async fn get_balance_for_user_address(&self, user_id: i32, address: &str) -> Result<i64, eyre::Error>{
        let balance_record = sqlx::query!(
            r#"
            SELECT 
                COALESCE(SUM(o.value), 0)::BIGINT AS "balance!" 
            FROM utxos o
            INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
            WHERE 
                ta.address = $1 
                AND ta.user_id = $2 
                AND o.status = 'unspent'
            "#,
            address,
            user_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(balance_record.balance)
    }
    async fn get_transaction_history_for_user_address(&self, user_id: i32, address: &str) -> Result<Vec<UtxoResponse>, eyre::Error>{
        
        let transactions = sqlx::query_as!(
            UtxoResponse,
            r#"
            SELECT 
                o.txid, o.vout, o.value, t.block_height
            FROM utxos o  
            INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
            INNER JOIN transactions t ON o.txid = t.txid
            WHERE 
                ta.address = $1 
                AND ta.user_id = $2 
            ORDER BY t.block_height DESC NULLS FIRST, o.txid
            "#,
            address,
            user_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(transactions)
      
    }
}