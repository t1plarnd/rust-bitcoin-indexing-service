# Bitcoin Indexer in Rust

This project indexes Bitcoin transactions using P2P SPV-like Node with access to mempool transactions, fetch them into a PostgreSQL database and provides a REST API to retrieve them. Also service as fault tolerance mothod suports connection to Blockstream.info, as alternative.
The project uses `axum`, `sqlx`, `rust-bitcoin`, `nginx` and `docker-compose`.

## How to Run

Uses Docker Compose to run the entire stack (API, Database, Nginx).

1.  **Requirements:**
    * Docker
    * Docker Compose

2.  **Clone the repository:**
    ```bash
    git clone https://github.com/t1plarnd/rust-bitcoin-indexing-service.git
    cd rust-bitcoin-indexing-service
    ```

3.  **Create the .env file:**
    Copy the env.example template and fill it with your data.
    ```bash
    cp .env.example .env
    ```
    You need to edit .env 

4.  **Start the service:**
    ```bash
    docker compose up --build
    ```

5.  **That's it!** The service is now available at http://localhost.

## API Endpoints

* POST/addresses/balance - Get balance of all unspent utxos for user by list of addresses
* POST /addresses/utxos - Get list of active(unspen) utxos for user by list of addresses
* POST /addresses/txs - Get transaction history for user by list of addresses
* POST /addresses - Set new address to track for user
* GET /addresses - Get list of addresses tracked by user
* POST /register - Create new user
* POST /login - Login into existing user
