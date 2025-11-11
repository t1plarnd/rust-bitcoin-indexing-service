use axum::{
    body::Body,
    extract::{Extension, State}, 
        extract::Path,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use db::{DbRepository, TrackedAddress}; 
use eyre::Result;
use jsonwebtoken::{encode, decode, Header, EncodingKey, DecodingKey, Validation};
use serde::{Deserialize}; 
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{CorsLayer, Any};
use models::{Config, RegisterData, Claims, UtxoResponse};
use serde_json::json;


#[derive(Debug, Deserialize)]
pub struct TrackAddressRequest {
    pub address: String,
}
#[derive(Clone)]
pub struct AppState {
    pub db_repo: Arc<dyn DbRepository>,
    pub config: Arc<Config>, 
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
                eprintln!("Internal server error: {:?}", err);
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



pub async fn run(app_state: AppState) -> Result<()> {
    
    let public_routes = Router::new()
        .route("/login", post(login))
        .route("/register", post(register));
        
    let protected_routes = Router::new()
        .route("/addresses/:address/balance", get(get_balance))
        .route("/addresses/:address/utxos", get(get_utxos))
        .route("/addresses/:address/txs", get(get_transaction_history))
        .route("/addresses", get(get_tracked_addr)) 
        .route("/addresses", post(track_new_address)) 
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware
        ));

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .with_state(app_state)
        .layer(CorsLayer::new().allow_origin(Any));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("API server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}


#[axum::debug_handler]
async fn register(
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
async fn login(
    State(state): State<AppState>,
    Json(data): Json<RegisterData>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let is_valid_user = state.db_repo.check_user(&data).await?;
    if is_valid_user {
        let now = Utc::now();
        let expires_in = Duration::hours(24);   
        let user_id = state.db_repo.get_user_id_by_username(&data.username).await?;
        let claims = Claims {
            sub: user_id.to_string(), // Кладемо user_id у `sub` (subject)
            exp: (now + expires_in).timestamp(),
        };

        let token = encode(
            &Header::default(), 
            &claims, 
            &EncodingKey::from_secret(state.config.jwt_secret.as_ref()) // ВИПРАВЛЕНО
        )
        .map_err(|e| ApiError::Internal(eyre::eyre!("Token creation error: {}", e)))?;

        Ok(Json(json!({ "token": token })))
    } else {
        Err(ApiError::Unauthorized("Invalid username or password".to_string()))
    }
}
#[axum::debug_handler]
async fn get_balance(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> Result<Json<i64>, ApiError> {  

    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;


    let balance = state.db_repo.get_balance_for_user_address(user_id, &address).await
        .map_err(ApiError::from)?;

    Ok(Json(balance)) 
}
#[axum::debug_handler]
async fn get_utxos(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> Result<Json<Vec<UtxoResponse>>, ApiError> {
    
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;
    let utxos = state.db_repo.get_utxos_for_user_address(user_id, &address).await;

    Ok(Json(utxos?))
}
#[axum::debug_handler]
async fn get_transaction_history(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> Result<Json<Vec<UtxoResponse>>, ApiError> { 
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;

        let transactions = state.db_repo.get_transaction_history_for_user_address(user_id, &address).await;

    Ok(Json(transactions?))
}
#[axum::debug_handler]
async fn track_new_address(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TrackAddressRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;

    state.db_repo.add_tracked_address(user_id, &payload.address).await?;

    Ok(Json(json!({
        "status": "success",
        "message": "Address added. Indexer will scan it shortly."
    })))
}
#[axum::debug_handler]
async fn get_tracked_addr(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<TrackedAddress>>, ApiError> { 
    
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;

    let addresses = state.db_repo.get_addresses_for_user(user_id).await?;

    Ok(Json(addresses))
}



pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    
    let auth_header = req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok());

    let token = if let Some(auth_header) = auth_header {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            token
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let claims = match decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_ref()), // ВИПРАВЛЕНО
        &Validation::default()
    ) {
        Ok(token_data) => token_data.claims,
        Err(_) => {
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}