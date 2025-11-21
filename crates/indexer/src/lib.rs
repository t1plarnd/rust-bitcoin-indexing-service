use bitcoin::BlockHeader;
use eyre::Result;
use bitcoin_hashes::{sha256d, Hash};
use tracing::{info, error, warn};
use db::responses::{NewTransaction, UtxoResponse};
use db::models::{
    AppState, 
    Config, 
    MerkleProof, 
    BlockstreamClient};
use serde::{Deserialize, Serialize};

pub async fn run_indexer(state: AppState) -> Result<()> {
    let api = BlockstreamClient::new(state.config.network); 
    let mut last_height = state.db_repo.get_last_indexed_height().await?; 
    let mut last_block_hash = state.db_repo.get_last_indexed_block_hash(last_height).await?;

    loop {
        let curr_height = api.get_tip_height().await?;
        if last_height < curr_height {
            let next_height = last_height + 1;
            let hash = api.get_block_hash(next_height).await?;
            let header = api.get_block_header(&hash).await?;
            if header.prev_blockhash.to_string() != last_block_hash {
                warn!("Blockchain reorganization detected at height {}", next_height);

                let list = state.db_repo.get_all_tracked_addresses().await?;
                let safe_height = if last_height > 6 { last_height - 6 } else { 0 };
                state.db_repo.delete_transactions_and_utxos(list, safe_height as i32).await?;
                last_height = safe_height;
                last_block_hash = api.get_block_hash(safe_height).await?; 
                
                continue;
            }
            let target = header.target();
            if let Err(e) = header.validate_pow(&target) {
                error!("Invalid PoW for block {}: {}", next_height, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                continue;
            }

            info!("Header {} is valid. Checking transactions...", next_height);
            process_block(&state, &api, &hash, &header).await?;


            last_height = next_height;
            last_block_hash = hash;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            
        } else {
            info!("Synced at height {}. Sleeping...", last_height);
            tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
        }
    }
}

async fn process_block(state: &AppState, api: &BlockstreamClient, target_block_hash: &str, header: &BlockHeader) -> Result<()> {
    
    let addresses = state.db_repo.get_all_tracked_addresses().await?;
    
    for tracked_addr in addresses {
        let history = api.get_address_txs(&tracked_addr.address).await?;
        for tx in history {
            let is_in_this_block = match &tx.status.block_hash {
                Some(h) => h == target_block_hash, 
                None => false,
            };
            if !is_in_this_block { continue; }

            info!("Found tx {} in block {}", tx.txid, target_block_hash);
            let proof = api.get_merkle_proof(&tx.txid).await?;
            let expected_root = header.merkle_root.to_string();

            if verify_merkle_proof(&tx.txid, &proof, &expected_root) {
                info!("Merkle Proof Valid. Saving to DB.");
                let vouts_indices: Vec<i32> = (0..tx.vout.len()).map(|i| i as i32).collect();
                let vins_indices: Vec<i32> = tx.vin.iter().map(|v| v.vout as i32).collect();
                let new_tx = NewTransaction {
                    txid: tx.txid.clone(),
                    block_height: tx.status.block_height,
                    block_hash: tx.status.block_hash.clone(),
                    block_time: tx.status.block_time.map(|t| 
                        chrono::DateTime::from_timestamp(t, 0).unwrap().naive_utc()
                    ),
                    vouts: Some(vouts_indices), 
                    vins: Some(vins_indices),
                };
                let mut utxos_to_save = Vec::new();
                for (idx, vout_data) in tx.vout.iter().enumerate() {
                    if let Some(addr) = &vout_data.scriptpubkey_address {
                        if addr == &tracked_addr.address {
                            utxos_to_save.push(UtxoResponse {
                                address_id: tracked_addr.address_id,
                                txid: tx.txid.clone(),
                                vout_idx: idx as i32,   
                                value: vout_data.value as i64, 
                                block_height: tx.status.block_height,
                                is_spent: false,
                            });
                        }
                    }
                }
                state.db_repo.save_transaction_and_utxos(new_tx, utxos_to_save).await?;
                for input in tx.vin {
                    if let Some(prevout) = &input.prevout {
                        if let Some(addr) = &prevout.scriptpubkey_address {
                            if addr == &tracked_addr.address {
                                state.db_repo.mark_utxo_spent(&input.txid, input.vout as i32).await?;
                            }
                        }
                    }
                }
            } else {
                error!("Merkle proof failed for tx {}", tx.txid);
            }
        }
    }
    Ok(())
}

fn verify_merkle_proof(txid_hex: &str, proof: &MerkleProof, expected_root_hex: &str) -> bool {
    let mut current_hash = hex::decode(txid_hex).unwrap();
    current_hash.reverse();
    let mut pos = proof.pos;

    for partner_hex in &proof.merkle {
        let mut partner = hex::decode(partner_hex).unwrap();
        partner.reverse();
        let mut data_to_hash = Vec::new();
        if pos % 2 == 0 {
            data_to_hash.extend_from_slice(&current_hash);
            data_to_hash.extend_from_slice(&partner);
        } else {
            data_to_hash.extend_from_slice(&partner);
            data_to_hash.extend_from_slice(&current_hash);
        }
        let hash = sha256d::Hash::hash(&data_to_hash);
        current_hash = hash.to_byte_array().to_vec();

        pos /= 2;
    }

    current_hash.reverse(); 
    let calculated_root = hex::encode(current_hash);
    calculated_root == expected_root_hex
}