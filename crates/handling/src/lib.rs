use axum::{
    body::Body,
    extract::{Extension, State}, 
        extract::Path,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use db::{DbRepository, TrackedAddress, RegisterData, UtxoResponse, AddressPayload}; 
use std::sync::Arc;
use serde::{Deserialize, Serialize}; 
use serde_json::json;
use chrono::{Duration, Utc};
use eyre::Result;
use jsonwebtoken::{encode, Header, EncodingKey};
use std::path::PathBuf;
use dotenv::dotenv;
use std::env;
use tracing::error;

#[derive(Clone)]
pub struct AppState {
    pub db_repo: Arc<dyn DbRepository>,
    pub config: Arc<Config>, 
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String, 
    pub exp: i64,  
}
#[derive(Debug, Deserialize)]
pub struct TrackAddressRequest {
    pub address: String,
}
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub nakamoto_path: PathBuf,
    pub port: u16,
    pub ip_musk: Option<String>,
    
}
impl Config {
    pub fn load() -> Result<Self> {
        dotenv().ok();
        let database_url = env::var("DATABASE_URL")?;
        let jwt_secret = env::var("JWT_STRING")?;
        let nakamoto_path = env::var("NAKAMOTO_PATH")?;
        let port: u16 = env::var("API_PORT")?.parse()?;
        let ip_musk = env::var("IP_MUSK").ok();
        Ok(Config {
            database_url,
            jwt_secret,
            nakamoto_path: nakamoto_path.into(),
            port,   
            ip_musk,  
        })
    }
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
        let body = Json(json!({ "error": error_message }));
        (status, body).into_response()
    }
}
impl From<eyre::Error> for ApiError {
    fn from(err: eyre::Error) -> Self {
        ApiError::Internal(err)
    }
}

#[axum::debug_handler]
pub async fn register(
    State(state): State<AppState>,
    Json(data): Json<RegisterData>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.db_repo.create_user(&data).await? {
        true => { 
        Ok(Json(json!({ "status": "success", "message": "User registered" })))
        }
        false => { 
        Err(ApiError::Conflict("User with this username already exists".to_string()))
        }
    }
}

#[axum::debug_handler]
pub async fn login(
    State(state): State<AppState>,
    Json(data): Json<RegisterData>,
) -> Result<Json<serde_json::Value>, ApiError> {

    let is_valid_user = state.db_repo.check_user(&data).await?;

    if is_valid_user {
        let now = Utc::now();
        let expires_in = Duration::hours(24);   
        let user_id = state.db_repo.get_user_id_by_username(&data.username).await?;
        let claims = Claims {
            sub: user_id.to_string(), 
            exp: (now + expires_in).timestamp(),
        };

        let token = encode(
            &Header::default(), 
            &claims, 
            &EncodingKey::from_secret(state.config.jwt_secret.as_ref()) 
        )
        .map_err(|e| ApiError::Internal(eyre::eyre!("Token creation error: {}", e)))?;

        Ok(Json(json!({ "token": token })))
    } 
    else {
        Err(ApiError::Unauthorized("Invalid username or password".to_string()))
    }
}

#[axum::debug_handler]
pub async fn get_balance(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> Result<Json<i64>, ApiError> {  

    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let balance = state.db_repo.get_balance_for_user_address(user_id, &address).await?;

    Ok(Json(balance)) 
}

#[axum::debug_handler]
pub async fn get_utxos(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> Result<Json<Vec<UtxoResponse>>, ApiError> {
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let utxos = state.db_repo.get_utxos_for_user_address(user_id, &address).await;

    Ok(Json(utxos?))
}

#[axum::debug_handler]
pub async fn get_transaction_history(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<AddressPayload>,
) -> Result<Json<Vec<UtxoResponse>>, ApiError> { 

    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let has_access = state.db_repo.check_user_address_access(user_id, payload).await?;
    let transactions = state.db_repo.get_transaction_history_for_user_addresses(user_id, has_access).await;

    Ok(Json(transactions?))
}

#[axum::debug_handler]
pub async fn track_new_address(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TrackAddressRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    state.db_repo.add_tracked_address(user_id, &payload.address).await?;

    Ok(Json(json!({
        "status": "success",
        "message": "Address added. Indexer will scan it shortly."
    })))
}

#[axum::debug_handler]
pub async fn get_tracked_addr(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<TrackedAddress>>, ApiError> { 
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let addresses = state.db_repo.get_addresses_for_user(user_id).await?;

    Ok(Json(addresses))
}

pub async fn get_user_id(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<i32, ApiError> { 
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;
    Ok(user_id)
}