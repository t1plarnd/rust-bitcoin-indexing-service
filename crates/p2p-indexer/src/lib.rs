use std::net::{SocketAddr, IpAddr};
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::Cursor;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::message_blockdata::{GetHeadersMessage, Inventory};
use bitcoin::p2p::ServiceFlags;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::Network;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use bitcoin::Address;
use tokio::net::TcpStream;
use tokio::io::{
    AsyncReadExt, 
    AsyncWriteExt, 
    BufReader};
use tokio::sync::Mutex;
use eyre::Result;
use tracing::{info, warn, debug};
use chrono::NaiveDateTime;
use db::models::{
    AppState, 
    UtxoPayload, 
    VinJson, 
    VoutJson};

const DNS_SEEDS: &[&str] = &[
    "seed.bitcoin.sipa.be", 
    "dnsseed.bluematt.me",
    "dnsseed.bitcoin.dashjr.org",
    "seed.bitcoinstats.com",
];
const MAX_MSG_SIZE: usize = 8 * 1024 * 1024; 
type ActivePeers = Arc<Mutex<HashSet<SocketAddr>>>;

pub async fn run_p2p_indexer(state: AppState) -> Result<()> {
    info!("Starting P2P Indexer");
    
    let active_peers: ActivePeers = Arc::new(Mutex::new(HashSet::new()));
    
    loop { 
        let peer_addresses = discover_peers().await;
        let mut potential_peers = Vec::new();
        {
            let active = active_peers.lock().await;
            for addr in peer_addresses {
                if !active.contains(&addr) {
                    potential_peers.push(addr);
                }
            }
        }

        info!("Found {} new potential peers.", potential_peers.len());

        let mut connection_tasks = vec![];
        for addr in potential_peers.into_iter().take(4) {
            let active_peers_clone = active_peers.clone();
            let state_clone = state.clone();
            let task = tokio::spawn(async move {{
                    active_peers_clone.lock().await.insert(addr);
                }
                match handle_peer(addr, state_clone).await {
                    Ok(_) => info!("Connection to {} finished normally.", addr),
                    Err(e) => warn!("Connection to {} lost/failed: {}", addr, e),
                }
                {
                    active_peers_clone.lock().await.remove(&addr);
                }
            });
            connection_tasks.push(task);
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }
}
async fn discover_peers() -> Vec<SocketAddr> {
    let mut addresses = HashSet::new();
    let fallbacks = vec![
        "157.245.121.233:8333", 
        "95.216.21.47:8333",
        "5.9.148.139:8333",
    ];
    for ip in fallbacks {
        if let Ok(addr) = ip.parse::<SocketAddr>() {
            addresses.insert(addr);
        }
    }
    for seed in DNS_SEEDS {
        match tokio::net::lookup_host(format!("{}:8333", seed)).await {
            Ok(mut lookup) => {
                while let Some(addr) = lookup.next() {
                    addresses.insert(addr);
                }
            }
            Err(e) => {
                debug!("DNS seed {} lookup failed: {}", seed, e);
            }
        }
    }
    addresses.into_iter().collect()
}
async fn handle_peer(addr: SocketAddr, state: AppState) -> Result<()> {
    info!("Connecting to {}...", addr);
    
    let mut stream = match tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(eyre::eyre!("Connection timeout")),
    };

    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let my_addr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)), 0);
    let version_msg = VersionMessage {
        version: 70016, 
        services: ServiceFlags::WITNESS,
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64,
        receiver: bitcoin::p2p::address::Address::new(&addr, ServiceFlags::NONE),
        sender: bitcoin::p2p::address::Address::new(&my_addr, ServiceFlags::NONE),
        nonce: rand::random(),
        user_agent: "/RustFullIndexer:0.1/".to_string(),
        start_height: 0,
        relay: false, 
    };
    send_msg(&mut writer, NetworkMessage::Version(version_msg)).await?;

    let info_record = state.db_repo.get_info(state.config.last_height, state.config.last_height_hash).await?;
    let mut current_hash = info_record.hash_p2p;
    let mut current_height = info_record.height;
    let mut tracked_list = state.db_repo.get_all_tracked_addresses().await?;
    let mut address_map: HashMap<String, i32> = tracked_list
        .iter()
        .map(|t| (t.address.clone(), t.address_id))
        .collect();
    let mut last_refresh = std::time::Instant::now();

    loop {
        if last_refresh.elapsed().as_secs() > 15 {
            if let Ok(new_list) = state.db_repo.get_all_tracked_addresses().await {
                if new_list.len() != tracked_list.len() {
                    info!("Watchlist updated");
                    address_map = new_list
                        .iter()
                        .map(|t| (t.address.clone(), t.address_id))
                        .collect();
                    tracked_list = new_list;
                }
            }
            last_refresh = std::time::Instant::now();
        }

        let msg_result = tokio::time::timeout(
            std::time::Duration::from_secs(30), 
            read_msg(&mut reader)
        ).await;

        let raw_msg = match msg_result {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {continue;}
        };

        match raw_msg.payload() {
            NetworkMessage::Version(_) => {
                send_msg(&mut writer, NetworkMessage::Verack).await?;
            }
            NetworkMessage::Verack => {
                request_next_header(&mut writer, current_hash).await?;
            }
            NetworkMessage::Headers(headers) => {
                if headers.is_empty() {
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    continue;
                }
                let header = &headers[0];
                let header_hash = header.block_hash();

                if header.validate_pow(header.target()).is_err() {
                    warn!("Invalid PoW for block {}", header_hash);
                    return Ok(()); 
                }

                info!("Received header {}. Downloading full block...", header_hash);

                let inv = Inventory::WitnessBlock(header_hash);
                send_msg(&mut writer, NetworkMessage::GetData(vec![inv])).await?;
            }
            NetworkMessage::Block(block) => {
                let block_hash = block.block_hash();
                process_p2p_block(&state, &block, current_height + 1, &address_map).await?;
                current_height += 1;
                current_hash = block_hash;
                state.db_repo.save_info(current_height, block_hash.to_string()).await?;
                
                info!("Processed block {}. Height: {}", block_hash, current_height);
                request_next_header(&mut writer, block_hash).await?;
            }
            NetworkMessage::Ping(nonce) => {
                send_msg(&mut writer, NetworkMessage::Pong(*nonce)).await?;
            }
            NetworkMessage::Inv(invs) => {
                info!("Received Inv: {} items", invs.len());
            }
            _ => {}
        }
    }
}
async fn process_p2p_block(state: &AppState, block: &bitcoin::Block, height: u32, address_map: &HashMap<String, i32>) -> Result<()> {
    let block_hash_str = block.block_hash().to_string();
    let block_time = NaiveDateTime::from_timestamp_opt(block.header.time as i64, 0).unwrap_or_default();
    
    let mut list_to_save: Vec<UtxoPayload> = Vec::new();

    for tx in &block.txdata {
        let txid = tx.txid().to_string();
        let json_vins: Vec<VinJson> = tx.input.iter().map(|vin| VinJson {
            txid: vin.previous_output.txid.to_string(),
            vout: vin.previous_output.vout,
            scriptsig: vin.script_sig.to_hex_string(), 
            sequence: vin.sequence.to_consensus_u32() as u64,
        }).collect();

        let mut json_vouts: Vec<VoutJson> = Vec::new();
        let mut relevant_outputs: Vec<(usize, String, u64)> = Vec::new(); 

        for (idx, out) in tx.output.iter().enumerate() {
            let mut addr_str = None;
            if let Ok(addr) = Address::from_script(&out.script_pubkey, Network::Bitcoin) {
                let s = addr.to_string();
                addr_str = Some(s.clone());
                relevant_outputs.push((idx, s, out.value.to_sat()));
            }
            json_vouts.push(VoutJson {
                n: idx,
                value: out.value.to_sat(),
                scriptpubkey: out.script_pubkey.to_hex_string(),
                address: addr_str,
            });
        }
        for (idx, addr, val) in relevant_outputs {
            if let Some(&addr_id) = address_map.get(&addr) {
                list_to_save.push(UtxoPayload {
                    address_id: addr_id,
                    txid: txid.clone(),
                    vout_idx: idx as i32,
                    block_hash: block_hash_str.clone(),
                    block_time,
                    vouts: json_vouts.clone(), 
                    vins: json_vins.clone(),  
                    value: val as i64,
                    block_height: height as i32,
                });
            }
        }
        for vin in &tx.input {
            if !vin.previous_output.is_null() {
                let prev_txid = vin.previous_output.txid.to_string();
                let prev_vout = vin.previous_output.vout;
                if let Err(e) = state.db_repo.mark_utxo_spent(&prev_txid, prev_vout as i32).await {
                     debug!("Failed to mark spent {}:{}: {}", prev_txid, prev_vout, e);
                }
            }
        }
    }
    if !list_to_save.is_empty() {
        info!("Saving {} new UTXOs from block {}", list_to_save.len(), height);
        state.db_repo.save_utxos(list_to_save).await?;
    }

    Ok(())
}
async fn send_msg<W: AsyncWriteExt + Unpin>(writer: &mut W, payload: NetworkMessage) -> Result<(), std::io::Error> {
    let magic = Network::Bitcoin.magic(); 
    let raw = RawNetworkMessage::new(magic, payload);
    let mut buffer = Vec::new();
    raw.consensus_encode(&mut buffer).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    writer.write_all(&buffer).await?;
    writer.flush().await?;
    Ok(())
}
async fn read_msg<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<RawNetworkMessage, std::io::Error> {
    let mut header_buf = [0u8; 24];
    reader.read_exact(&mut header_buf).await?;
    let payload_len = u32::from_le_bytes(header_buf[16..20].try_into().unwrap()) as usize;
    if payload_len > MAX_MSG_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData, 
            format!("Message size too large: {} bytes", payload_len)
        ));
    }
    let mut payload_buf = vec![0u8; payload_len];
    reader.read_exact(&mut payload_buf).await?;
    let mut total_msg = Vec::with_capacity(24 + payload_len);
    total_msg.extend_from_slice(&header_buf);
    total_msg.extend_from_slice(&payload_buf);
    
    let raw_msg: RawNetworkMessage = Decodable::consensus_decode(&mut Cursor::new(total_msg))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    
    Ok(raw_msg)
}
async fn request_next_header<W: AsyncWriteExt + Unpin>(writer: &mut W, last_hash: BlockHash) -> Result<()> {
    let locator_hashes = vec![last_hash]; 
    let get_headers_msg = GetHeadersMessage {
        version: 70016,
        locator_hashes,
        stop_hash: BlockHash::all_zeros(), 
    };
    send_msg(writer, NetworkMessage::GetHeaders(get_headers_msg)).await?;
    Ok(())
}