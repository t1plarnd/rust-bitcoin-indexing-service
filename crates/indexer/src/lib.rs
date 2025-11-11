use std::{net::TcpStream, path::PathBuf, sync::Arc, thread};
use std::time::{Duration, Instant};
use crossbeam_channel::RecvTimeoutError;
use db::DbRepository;
use eyre::Result;
use nakamoto_client::{Client, Config, Network};
use nakamoto_client::handle::Handle;
use nakamoto_net_poll::Reactor;
use bitcoin::{Address, Script};
use std::str::FromStr;
use tokio::runtime::Handle as TokioHandle;
use models::{NewTransaction, NewTxOutput};

pub async fn run_indexer(db_repo: Arc<dyn DbRepository>, network: Network, root: PathBuf) -> Result<()> {
    println!("Starting Nakamoto SPV client on {:?}...", network);
    let config = Config { 
        root, 
        network,
        ..Config::default() 
    };
    let client = Client::<Reactor<TcpStream>>::new()?;
    let handle = client.handle();
    let filters_receiver = Handle::filters(&handle);
    let blocks_receiver = Handle::blocks(&handle);

    thread::spawn(move || {
        if let Err(e) = client.run(config) {
             eprintln!("Nakamoto client exited with error: {}", e);
        }
    });

    let mut tracked_scripts: std::collections::HashMap<Script, i32> = std::collections::HashMap::new();
    let mut last_address_sync = Instant::now();
    let address_sync_interval = Duration::from_secs(30);
    let mut latest_seen_height: u64 = 900_000;
    println!("Nakamoto started. Waiting for events...");
    let handle_for_commands = handle.clone();

    tokio::task::spawn_blocking(move || {
        loop {
            let now = Instant::now();
            if now.duration_since(last_address_sync) > address_sync_interval {
                let new_addresses = TokioHandle::current().block_on(db_repo.get_all_tracked_addresses());
                if let Ok(addresses) = new_addresses {
                    let mut new_scripts_count = 0;
                    for record in addresses {
                        if let Ok(addr) = Address::from_str(&record.address) {
                            let script = addr.script_pubkey();
                            if !tracked_scripts.contains_key(&script) {
                                tracked_scripts.insert(script, record.address_id);
                                new_scripts_count += 1;
                            }
                        }
                    }
                    if new_scripts_count > 0 {
                        println!("Found {} new addresses. Total tracked: {}", new_scripts_count, tracked_scripts.len());
                        let scripts_to_watch = tracked_scripts.keys().cloned().collect::<Vec<_>>();
                        if let Err(e) = handle_for_commands.watch(scripts_to_watch.clone().into_iter()) {
                            eprintln!("Error updating watchlist: {}", e);
                        }
                        let rescan_range =  500_000u64..=latest_seen_height;
                        if let Err(e) = handle_for_commands.rescan(rescan_range, scripts_to_watch.into_iter()) {
                            eprintln!("Error requesting rescan: {}", e);
                        } 
                        else println!("Requested rescan up to height {}", latest_seen_height);
                    }
                }
                last_address_sync = Instant::now();
            }
            match filters_receiver.recv_timeout(Duration::from_secs(1)) {
                Ok((filter, block_hash, height)) => {
                    latest_seen_height = latest_seen_height.max(height);
                    let mut scripts_iter = tracked_scripts.keys().map(|s| s.as_bytes());
                    match filter.match_any(&block_hash, &mut scripts_iter) {
                        Ok(true) => {
                            println!(" Filter matched for block {} at height {}", block_hash, height);
                            if let Err(e) = handle_for_commands.get_block(&block_hash) {
                                eprintln!("Error requesting block {}: {}", block_hash, e);
                            }
                        }
                        Ok(false) => {}
                        Err(e) => {
                            eprintln!("Error matching filter for block {}: {}", block_hash, e);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            match blocks_receiver.recv_timeout(Duration::from_secs(1)) {
                Ok((block, height)) => {
                    let block_h = height as u64;
                    latest_seen_height = latest_seen_height.max(block_h);
                    println!(" Block received at height {} with {} transactions", height, block.txdata.len());
                    let mut fork_height: Option<u64> = None;
                    match handle_for_commands.find_branch(&block.block_hash()) {
                        Ok(Some((h, _headers))) => {
                            fork_height = Some(h as u64);
                        }
                        Ok(None) => {
                        }
                        Err(e) => {
                            eprintln!("find_branch error: {}", e);
                        }
                    }
                    let fork_h = fork_height.unwrap_or_else(|| block_h.saturating_sub(6));
                    let db_clone = db_repo.clone();
                    let tracked_scripts_clone = tracked_scripts.keys().cloned().collect::<Vec<_>>();
                    let need_reconcile = TokioHandle::current().block_on(async move {
                        match db_clone.get_txids_since(fork_h as i32).await {
                            Ok(txids) => {
                                if !txids.is_empty() {
                                    if let Err(e) = db_clone.delete_transactions_and_utxos(&txids).await {
                                        eprintln!("DB error deleting txs during reorg reconcile: {}", e);
                                    }
                                    true
                                } else {
                                    false
                                }
                            }
                            Err(e) => {
                                eprintln!("DB error checking txids for reorg: {}", e);
                                false
                            }
                        }
                    });
                    if need_reconcile {
                        let rescan_range = fork_h..=latest_seen_height;
                        if let Err(e) = handle_for_commands.rescan(rescan_range, tracked_scripts_clone.into_iter()) {
                            eprintln!("Error requesting rescan during reorg reconcile: {}", e);
                        } else {
                            println!("Requested rescan for fork range {}..={}", fork_h, latest_seen_height);
                        }
                    }
                    process_block(&block, height.try_into().unwrap(), &tracked_scripts, &db_repo);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }).await?;

    Ok(())
}

fn process_block(
    block: &bitcoin::Block,
    height: u32,
    tracked_scripts: &std::collections::HashMap<Script, i32>,
    db_repo: &Arc<dyn DbRepository>
) {
    for tx in &block.txdata {
        let txid = tx.txid().to_string();
        let mut my_outputs = Vec::new();

        for input in &tx.input {
            if input.previous_output.is_null() {
                continue;
            }
            let prev_txid = input.previous_output.txid.to_string();
            let prev_vout = input.previous_output.vout as i32;
            let db = Arc::clone(db_repo);
            let _ = TokioHandle::current().block_on(async move {
                if let Err(e) = db.mark_utxo_spent(&prev_txid, prev_vout).await {
                    eprintln!("DB Error marking utxo spent {}:{}: {}", prev_txid, prev_vout, e);
                }
            });
        }
        for (vout, output) in tx.output.iter().enumerate() {
            if let Some(&address_id) = tracked_scripts.get(&output.script_pubkey) {
                println!("FOUND TX: {} | vout: {} | value: {}", txid, vout, output.value);
                my_outputs.push(NewTxOutput {
                    address_id,
                    txid: txid.clone(),
                    vout: vout as i32,
                    value: output.value as i64,
                });
            }
        }
        if !my_outputs.is_empty() {
            let new_tx = NewTransaction {
                txid: txid.clone(),
                block_height: Some(height as i32),
                block_hash: Some(block.block_hash().to_string()),
                block_time: None,
            };

            let db = db_repo.clone();
            TokioHandle::current().block_on(async move {
                if let Err(e) = db.save_transaction(new_tx, &my_outputs).await {
                    eprintln!("DB Error saving tx {}: {}", txid, e);
                }
                else {
                    println!(" Transaction {} saved to DB", txid);
                }
            });
        }
    }
}