use axum::{
    extract::{Extension, State}, 
    Json,}; 
use chrono::{Duration, Utc};
use eyre::Result;
use jsonwebtoken::{encode, Header, EncodingKey};
use db::models::{AppState, Claims};
use db::responses::{
    ApiError,
    RegisterData, 
    BalanceResponse, 
    AddressPayload, 
    RegisterResponse, 
    LoginResponse, 
    AddAddressResponse,
    AddressUtxoResponse};

#[axum::debug_handler]
pub async fn register(State(state): State<AppState>, Json(data): Json<RegisterData>,) -> Result<Json<RegisterResponse>, ApiError> {
    match state.db_repo.create_user(&data).await? {
        true => { 
        Ok(Json(RegisterResponse{
            success: "Registration succsessful".to_string(),
        }))
        }
        false => { 
        Err(ApiError::Conflict("User with this username already exists".to_string()))
        }
    }
}

#[axum::debug_handler]
pub async fn login(State(state): State<AppState>, Json(data): Json<RegisterData>,) -> Result<Json<LoginResponse>, ApiError> {
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

        Ok(Json(LoginResponse{
            success: "Login succsessful".to_string(),
            token: token,
        }))
    } 
    else {
        Err(ApiError::Unauthorized("Invalid username or password".to_string()))
    }
}

#[axum::debug_handler]
pub async fn get_balance(State(state): State<AppState>, Extension(claims): Extension<Claims>, Json(payload): Json<AddressPayload>,) -> Result<Json<Vec<BalanceResponse>>, ApiError> {  

    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    if state.db_repo.check_user_address_access(user_id, payload.clone()).await? == true {
        let balance = state.db_repo.get_balance_for_user_addresses(user_id, payload.addresses).await?;

        Ok(Json(balance)) 
    }
    else{
        Err(ApiError::Unauthorized("You do not have access to one or more of the requested addresses".to_string()))
    }
}

#[axum::debug_handler]
pub async fn get_utxos(State(state): State<AppState>, Extension(claims): Extension<Claims>, Json(payload): Json<AddressPayload>,) -> Result<Json<Vec<AddressUtxoResponse>>, ApiError> {
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    if state.db_repo.check_user_address_access(user_id, payload.clone()).await? == true {
        let utxos = state.db_repo.get_utxos_for_user_address(user_id, payload.addresses).await;

        Ok(Json(utxos?))
    }
    else{
        Err(ApiError::Unauthorized("You do not have access to one or more of the requested addresses".to_string()))
    }
}

#[axum::debug_handler]
pub async fn get_transaction_history(State(state): State<AppState>, Extension(claims): Extension<Claims>, Json(payload): Json<AddressPayload>,) -> Result<Json<Vec<AddressUtxoResponse>>, ApiError> { 

    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    if state.db_repo.check_user_address_access(user_id, payload.clone()).await? == true {
        let transactions = state.db_repo.get_transaction_history_for_user_addresses(user_id, payload.addresses).await;

        Ok(Json(transactions?))
    }
    else{
        Err(ApiError::Unauthorized("You do not have access to one or more of the requested addresses".to_string()))
    }
}

#[axum::debug_handler]
pub async fn track_new_addresses(State(state): State<AppState>, Extension(claims): Extension<Claims>, Json(payload): Json<AddressPayload>,) -> Result<Json<AddAddressResponse>, ApiError> {
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let info = state.db_repo.add_tracked_addresses(user_id, payload.addresses).await?;
    if info == true {
        Ok(Json(AddAddressResponse {
            success: "Address saved to db".to_string() 
        }))
    }
    else{
        Err(ApiError::Conflict("Can't add address".to_string()))
    }
}

#[axum::debug_handler]
pub async fn get_tracked_addr(State(state): State<AppState>, Extension(claims): Extension<Claims>,) -> Result<Json<Vec<String>>, ApiError> { 
    
    let user_id = get_user_id(State(state.clone()), Extension(claims)).await?;
    let addresses = state.db_repo.get_addresses_for_user(user_id).await?;

    Ok(Json(addresses))
}

pub async fn get_user_id(State(_state): State<AppState>, Extension(claims): Extension<Claims>,) -> Result<i32, ApiError> { 
    let user_id = claims.sub.parse::<i32>()
        .map_err(|_| ApiError::Internal(eyre::eyre!("Invalid user_id in token claims")))?;
    Ok(user_id)
}