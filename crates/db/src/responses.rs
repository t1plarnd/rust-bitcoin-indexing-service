use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json as AxumJson};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use sqlx::types::chrono;
use tracing::error;

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterData {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterResponse {
    pub success: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoginResponse {
    pub success: String,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AddAddressResponse {
    pub success: String,
}

#[derive(Deserialize, Clone)]
pub struct AddressPayload {
    pub addresses: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BalanceResponse {
    pub address: String,
    pub balance: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackedAddress {
    pub address_id: i32,
    pub address: String,
}

#[derive(Debug)]
pub struct NewTransaction {
    pub txid: String,
    pub block_height: Option<i32>,
    pub block_hash: Option<String>,
    pub block_time: Option<chrono::NaiveDateTime>,
    pub vouts: Option<Vec<i32>>, 
    pub vins: Option<Vec<i32>>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct UtxoResponse {
    pub address_id: i32,
    pub txid: String,
    pub vout_idx: i32,
    pub value: i64,
    pub block_height: Option<i32>,
    pub is_spent: bool,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AddressUtxoResponse {
    pub address: String,       
    pub txid: String,
    pub vout_idx: i32,           
    pub value: i64,
    pub block_height: Option<i32>,
    pub is_spent: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub enum ApiError {
    Unauthorized(String),
    Conflict(String),
    Internal(eyre::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::Internal(err) => {
                error!("Internal server error: {:?}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
        };
        let body = AxumJson(json!({ "error": error_message }));
        (status, body).into_response()
    }
}

impl From<eyre::Error> for ApiError {
    fn from(err: eyre::Error) -> Self {
        ApiError::Internal(err)
    }
}