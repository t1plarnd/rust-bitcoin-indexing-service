use axum::{
    body::Body,
    extract::State, 
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router};
use eyre::Result;
use jsonwebtoken::{decode, DecodingKey, Validation};
use std::net::{
    IpAddr, 
    Ipv4Addr, 
    SocketAddr};
use handling::{
    login, 
    register, 
    get_balance, 
    get_utxos, 
    get_tracked_addr, 
    track_new_addresses, 
    get_transaction_history};
use db::models::{AppState, Claims};
use tracing::info;

pub async fn run(app_state: AppState) -> Result<()> {
    let public_routes = Router::new()
        .route("/login", post(login))
        .route("/register", post(register));    
    let protected_routes = Router::new()
        .route("/addresses/balance", post(get_balance))
        .route("/addresses/utxos", post(get_utxos))
        .route("/addresses/txs", post(get_transaction_history))
        .route("/addresses", get(get_tracked_addr)) 
        .route("/addresses", post(track_new_addresses)) 
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware
        ));
    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .with_state(app_state.clone());
    let port = app_state.config.port;
    let ip_addr: IpAddr = match &app_state.config.ip_musk {
        Some(ip_string) => {
            ip_string.parse()?
        },
        None => {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED) 
        }
    };
    let addr = SocketAddr::new(ip_addr, port);
    info!("API server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
pub async fn auth_middleware(State(state): State<AppState>, mut req: Request<Body>, next: Next,) -> Result<Response, StatusCode> {
    let auth_header = req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok());
    let token = if let Some(auth_header) = auth_header {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            token
        } 
        else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } 
    else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let claims = match decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_ref()), 
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


