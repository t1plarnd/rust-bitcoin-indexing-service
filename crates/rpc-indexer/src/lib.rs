use bitcoin::block::Header as BlockHeader;
use eyre::Result;
use bitcoin_hashes::{sha256d, Hash};
use tracing::{info, error, warn};
use db::models::{
    AppState, 
    BlockstreamClient, 
    MerkleProof, 
    UtxoPayload, 
    VinJson, 
    VoutJson};
use db::responses::TrackedAddress;
use sqlx::types::chrono::NaiveDateTime;

pub async fn run_rpc_indexer(state: AppState) -> Result<()> {
    info!("Starting P2P Indexer");
    let api = BlockstreamClient::new();
    let record = state.db_repo.get_info(state.config.last_height, state.config.last_height_hash).await?;
    let mut last_height = record.height;
    let mut last_block_hash = record.hash_rpc;
    let max_reorg_capacity: u32= state.config.reorg_limit;

    loop {
        let curr_height = api.get_tip_height().await?;
        if last_height < curr_height {
            let next_height = last_height + 1;
            let hash = api.get_block_hash(next_height).await?;
            let header = api.get_block_header(&hash).await?;
            if header.prev_blockhash.to_string() != last_block_hash {
                warn!("Blockchain reorganization detected at height {}", next_height);

                let mut counter: u32 = 0;
                let mut hash2 = header.prev_blockhash.to_string();
                let hash2_b = header.prev_blockhash.to_string();
                let mut hash1 = last_block_hash;

                while hash1 != hash2 {
                    if counter == max_reorg_capacity {panic!("Reorg is bigger than {} blocks!", max_reorg_capacity);}
                    counter+=1;
                    let mut extracoutner = 0;
                    hash2 = hash2_b.clone();

                    while hash1 != hash2{
                        if extracoutner == max_reorg_capacity{break;}
                        extracoutner+=1;
                        hash2 = api.get_block_header(&hash2).await?.prev_blockhash.to_string();
                        tokio::time::sleep(tokio::time::Duration::from_millis(228)).await;
                        if hash1 == hash2 {break;}
                    }
                    if hash1 == hash2 {break;}
                    hash1 = api.get_block_header(&hash1).await?.prev_blockhash.to_string();
                    tokio::time::sleep(tokio::time::Duration::from_millis(228)).await;
                }
                let list = state.db_repo.get_all_tracked_addresses().await?;
                state.db_repo.rollback_state(counter as u32).await?;
                last_height -= counter;
                last_block_hash = api.get_block_hash(last_height).await?; 
                continue;
            }

            let target = header.target();
            if let Err(e) = header.validate_pow(target) {
                error!("Invalid PoW for block {}: {}", next_height, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                continue;
            }

            info!("Header {} is valid. Checking transactions...", next_height);
            let tracked_addresses = state.db_repo.get_all_tracked_addresses().await?;
            process_block(&state, &api, &hash, &header, tracked_addresses).await?;
            state.db_repo.save_info(last_height, last_block_hash).await?;
            last_height = next_height;
            last_block_hash = hash;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            
        } else {
            info!("Synced at height {}. Sleeping...", last_height);
            tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
        }
    }
}
async fn process_block(state: &AppState, api: &BlockstreamClient, target_block_hash: &str, header: &BlockHeader, tracked_addresses: Vec<TrackedAddress>) -> Result<()> {
    for tracked_addr in tracked_addresses {
        let history = api.get_address_txs(&tracked_addr.address).await?;
        for tx in history {
            let is_in_this_block = match &tx.status.block_hash {
                Some(h) => h == target_block_hash, 
                None => false,
            };
            if !is_in_this_block { continue; }

            let proof = api.get_merkle_proof(&tx.txid).await?;
            let expected_root = header.merkle_root.to_string();

            if verify_merkle_proof(&tx.txid, &proof, &expected_root) {
                let json_vins: Vec<VinJson> = tx.vin.iter().map(|v| VinJson {
                    txid: v.txid.clone(),
                    vout: v.vout,
                    scriptsig: v.scriptsig.clone(),
                    sequence: v.sequence as u64,
                }).collect();

                let json_vouts: Vec<VoutJson> = tx.vout.iter().enumerate().map(|(i, v)| VoutJson {
                    n: i,
                    value: v.value,
                    scriptpubkey: v.scriptpubkey.clone(),
                    address: v.scriptpubkey_address.clone(),
                }).collect();

                let block_time = tx.status.block_time
                        .map(|t| NaiveDateTime::from_timestamp_opt(t, 0).unwrap_or_default()).unwrap_or_default();
                let block_height = tx.status.block_height.unwrap_or(0);
                let mut list_to_save: Vec<UtxoPayload> = Vec::new();
                for (idx, vout) in tx.vout.iter().enumerate() {
                    if let Some(addr) = &vout.scriptpubkey_address {
                        if addr == &tracked_addr.address {
                            
                            list_to_save.push(UtxoPayload {
                                address_id: tracked_addr.address_id,
                                txid: tx.txid.clone(),
                                vout_idx: idx as i32, 
                                block_hash: target_block_hash.to_string(),
                                block_time,
                                vouts: json_vouts.clone(),
                                vins: json_vins.clone(),
                                value: vout.value as i64,
                                block_height,
                                is_confirmed: true,
                            });
                        }
                    }
                }
                if !list_to_save.is_empty() {
                    state.db_repo.save_utxos(list_to_save).await?;
                }
                for input in tx.vin {
                    if let Some(prevout) = &input.prevout {
                        if let Some(addr) = &prevout.scriptpubkey_address {
                             if addr == &tracked_addr.address {
                                 state.db_repo.mark_utxo_spent(&input.txid, input.vout as i32).await?;
                             }
                        }
                    }
                }
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