use async_trait::async_trait;
use eyre::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Error as SqlxError, PgPool};
use tokio::task::spawn_blocking;
use bcrypt::{hash, verify};
use sqlx::FromRow;
use sqlx::Row;

#[derive(Deserialize)]
pub struct AddressPayload {
    pub addresses: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackedAddress {
    pub address_id: i32,
    pub address: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterData {
    pub username: String,
    pub password: String,
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
    async fn get_transaction_history_for_user_addresses(&self, user_id: i32, address: AddressPayload) -> Result<Vec<UtxoResponse>, eyre::Error>;
    async fn mark_utxo_spent(&self, txid: &str, vout: i32) -> Result<(), eyre::Error>;
    async fn get_txids_since(&self, height: i32) -> Result<Vec<String>, eyre::Error>;
    async fn delete_transactions_and_utxos(&self, txids: &[String]) -> Result<(), eyre::Error>;
    async fn check_user_address_access(&self, user_id: i32, payload: AddressPayload) -> Result<AddressPayload, eyre::Error>;
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

    async fn add_tracked_address(&self, user_id: i32, address: &str) -> Result<i32, eyre::Error> {
        let record = sqlx::query(
            "INSERT INTO tracked_addresses (user_id, address) 
             VALUES ($1, $2)
             ON CONFLICT (user_id, address) DO UPDATE SET address = $2
             RETURNING address_id"
        )
        .bind(user_id)
        .bind(address)
        .fetch_one(&self.pool)
        .await?;
        
        Ok(record.get("address_id"))
    }

    async fn get_addresses_for_user(&self, user_id: i32) -> Result<Vec<TrackedAddress>, eyre::Error> {
        let addresses = sqlx::query(
            "SELECT address_id, address 
             FROM tracked_addresses
             WHERE user_id = $1"
        )
        .bind(user_id)
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

    async fn save_transaction(&self, tx_data: NewTransaction, outputs: &[NewTxOutput]) -> Result<(), eyre::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO transactions (txid, block_height, block_hash, block_time)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (txid) DO NOTHING"
        )
        .bind(&tx_data.txid)
        .bind(tx_data.block_height)
        .bind(&tx_data.block_hash)
        .bind(tx_data.block_time)
        .execute(&mut *tx)
        .await?;
        for output in outputs {
            sqlx::query(
                "INSERT INTO utxos (address_id, txid, vout, value, status)
                 VALUES ($1, $2, $3, $4, 'unspent')
                 ON CONFLICT (txid, vout) DO NOTHING"
            )
            .bind(output.address_id)
            .bind(&output.txid)
            .bind(output.vout)
            .bind(output.value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn mark_utxo_spent(&self, txid: &str, vout: i32) -> Result<(), eyre::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE utxos
             SET status = 'spent'
             WHERE txid = $1 
                 AND vout = $2 
                 AND status = 'unspent'"
        )
        .bind(txid)
        .bind(vout)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(())
    }

    async fn get_utxos_for_user_address(&self, user_id: i32, address: &str) -> Result<Vec<UtxoResponse>, eyre::Error> {
        let utxos = sqlx::query(
            "SELECT o.txid, o.vout, o.value, t.block_height
             FROM utxos o  
             INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
             INNER JOIN transactions t ON o.txid = t.txid
             WHERE ta.address = $1 
                 AND ta.user_id = $2 
                 AND o.status = 'unspent' 
             ORDER BY t.block_height DESC NULLS FIRST, o.txid"
        )
        .bind(address)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| UtxoResponse {
            txid: row.get("txid"),
            vout: row.get("vout"),
            value: row.get("value"),
            block_height: row.get("block_height"),
        })
        .collect();

        Ok(utxos)
    }

    async fn get_balance_for_user_address(&self, user_id: i32, address: &str) -> Result<i64, eyre::Error> {
        let balance_record = sqlx::query(
            "SELECT COALESCE(SUM(o.value), 0)::BIGINT as balance
             FROM utxos o
             INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
             WHERE ta.address = $1 
                 AND ta.user_id = $2 
                 AND o.status = 'unspent'"
        )
        .bind(address)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(balance_record.get("balance"))
    }

    async fn get_transaction_history_for_user_addresses(&self, user_id: i32, addresses: AddressPayload) -> Result<Vec<UtxoResponse>, eyre::Error> {
        let mut transactions = Vec::new();
        for address in addresses.addresses {
            let query_results = sqlx::query(
                "SELECT o.txid, o.vout, o.value, t.block_height
                 FROM utxos o  
                 INNER JOIN tracked_addresses ta ON o.address_id = ta.address_id
                 INNER JOIN transactions t ON o.txid = t.txid
                 WHERE ta.address = $1 
                     AND ta.user_id = $2 
                 ORDER BY t.block_height DESC NULLS FIRST, o.txid"
            )
            .bind(&address)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| UtxoResponse {
                txid: row.get("txid"),
                vout: row.get("vout"),
                value: row.get("value"),
                block_height: row.get("block_height"),
            })
            .collect::<Vec<UtxoResponse>>();
            transactions.extend(query_results);
        }
    
        Ok(transactions)
    }

    async fn get_txids_since(&self, height: i32) -> Result<Vec<String>, eyre::Error> {
        let rows = sqlx::query(
            "SELECT txid 
             FROM transactions 
             WHERE block_height >= $1"
        )
        .bind(height)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| row.get("txid"))
        .collect();

        Ok(rows)
    }

    async fn delete_transactions_and_utxos(&self, txids: &[String]) -> Result<(), eyre::Error> {
        if txids.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM utxos WHERE txid = ANY($1)")
            .bind(txids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM transactions WHERE txid = ANY($1)")
            .bind(txids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        
        Ok(())
    }

    async fn check_user_address_access(&self, user_id: i32, payload: AddressPayload) -> Result<AddressPayload, eyre::Error> {
        let owned_addresses = sqlx::query(
            "SELECT address 
             FROM tracked_addresses 
             WHERE user_id = $1        
                 AND address = ANY($2)"
        )
        .bind(user_id)
        .bind(&payload.addresses)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| row.get("address"))
        .collect();

        Ok(AddressPayload {
            addresses: owned_addresses,
        })
    }
}