CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,
    username VARCHAR(100) NOT NULL UNIQUE,
    hashed_password VARCHAR(255) NOT NULL
);

CREATE INDEX idx_users_username ON users(username);

CREATE TABLE tracked_addresses (
    address_id SERIAL PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    address VARCHAR(255) NOT NULL,
    UNIQUE(user_id, address)
);
CREATE TABLE transactions (
    txid VARCHAR(64) PRIMARY KEY,
    block_height INTEGER,
    block_hash VARCHAR(64),
    block_time TIMESTAMP,
    vouts JSONB,
    vins JSONB
);
CREATE TABLE utxos (
    utxo_id SERIAL PRIMARY KEY,
    address_id INTEGER NOT NULL REFERENCES tracked_addresses(address_id) ON DELETE CASCADE,
    txid VARCHAR(64) NOT NULL REFERENCES transactions(txid) ON DELETE CASCADE,
    vout_idx INTEGER NOT NULL, 
    value BIGINT NOT NULL,
    block_height INTEGER,
    is_spent BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE(txid, vout_idx)
);
CREATE INDEX idx_utxos_address ON utxos(address_id);
CREATE INDEX idx_utxos_txid_vout ON utxos(txid, vout_idx);



