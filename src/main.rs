use std::{io::BufRead, sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    }, time::{Duration, SystemTime}};
use std::convert::TryInto;

use bitcoincash_addr::Address;
use block::{create_block, encode_compact_size, Block, GetBlockTemplateResponse};
use miner::{Miner, MiningSettings, Work};
use reqwest::RequestBuilder;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::sha256::lotus_hash;

mod block;
mod miner;
mod sha256;

struct Server {
    client: reqwest::Client,
    bitcoind_url: String,
    bitcoind_user: String,
    bitcoind_password: String,
    miner_addr: Address,
    miner: std::sync::Mutex<Miner>,
    block_state: Mutex<BlockState>,
    metrics_timestamp: Mutex<SystemTime>,
    metrics_nonces: AtomicU64,
    metrics_nonces_per_call: u64,
}

struct BlockState {
    current_work: Work,
    current_block: Option<Block>,
    next_block: Option<Block>,
    extra_nonce: u64,
}

type ServerRef = Arc<Server>;

fn n_bits_to_target(n_bits: u32) -> [u8; 32] {
    println!("n_bits: {:08x}, {}", n_bits, n_bits >> 24);
    let shift = (n_bits >> 24) - 3 - 16;
    let upper = ((n_bits & 0x7f_ffff) as u128) << (shift * 8);
    let mut target = [0u8; 32];
    target[16..].copy_from_slice(&upper.to_le_bytes());
    target
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mining_settings = MiningSettings {
        local_work_size: 256,
        inner_iter_size: 16,
        kernel_size: 1 << 22,
        kernel_name: "lotus_og".to_string(),
        sleep: 0,
        gpu_indices: vec![0],
    };
    let mut miner = Miner::setup(mining_settings.clone()).unwrap();
    let mut last_hash: Option<[u8; 32]> = None;
    let mut found_nonces = Vec::new();

    let file = std::fs::File::open("headers.txt")?;
    let mut headers = std::io::BufReader::new(file).lines();
    while let Some(header) = headers.next() {
        let mut header: [u8; 160] = hex::decode(&header?)?.try_into().unwrap();
        if let Some(last_hash) = last_hash {
            header[..32].copy_from_slice(&last_hash);
        }
        let n_bits = u32::from_le_bytes(header[32..36].try_into().unwrap());
        let target = n_bits_to_target(n_bits);
        println!("target: {}", hex::encode(&target));
        let mut work = Work::from_header(header, target);
        let mut big_nonce = 0u32;
        'solve_header: loop {
            work.set_big_nonce(big_nonce);
            println!("big nonce: {}", big_nonce);
            while miner.has_nonces_left(&work) {
                if let Some(nonce) = miner.find_nonce(&work).unwrap() {
                    let mut result_nonce = big_nonce as u64;
                    result_nonce <<= 32;
                    result_nonce |= nonce as u64;
                    work.set_nonce(nonce);
                    let mut solved_hash = lotus_hash(work.header());
                    println!("solved header: {}", hex::encode(work.header()));
                    found_nonces.push(result_nonce);
                    last_hash = Some(solved_hash);
                    solved_hash.reverse();
                    println!("{} has nonce: {}", hex::encode(&solved_hash), result_nonce);
                    break 'solve_header;
                }
                work.nonce_idx += 1;
            }
            big_nonce += 1;
            work.nonce_idx = 0;
        }
    }
    println!("Nonces:");
    for nonce in found_nonces {
        println!("    {}", nonce);
    }
    Ok(())
}

fn init_request(server: &Server) -> RequestBuilder {
    server
        .client
        .post(&server.bitcoind_url)
        .basic_auth(&server.bitcoind_user, Some(&server.bitcoind_password))
}

async fn update_next_block(server: &Server) -> Result<(), Box<dyn std::error::Error>> {
    let response = init_request(&server)
        .body(r#"{"method":"getblocktemplate","params":[]}"#)
        .send()
        .await?;
    let response = response.text().await?;
    let response: GetBlockTemplateResponse = serde_json::from_str(&response)?;
    let mut block_state = server.block_state.lock().await;
    let block = create_block(
        &server.miner_addr,
        &response.result,
        block_state.extra_nonce,
    );
    if let Some(current_block) = &block_state.current_block {
        if current_block.header[4..36] != block.header[4..36] {
            println!(
                "Switched to new chain tip: {}",
                response.result.previousblockhash
            );
        }
    } else {
        println!(
            "Started mining on chain tip: {}",
            response.result.previousblockhash
        );
    }
    block_state.extra_nonce += 1;
    block_state.next_block = Some(block);
    Ok(())
}

async fn mine_some_nonces(server: ServerRef) -> ocl::Result<()> {
    let mut block_state = server.block_state.lock().await;
    if let Some(next_block) = block_state.next_block.take() {
        block_state.current_work = Work::from_header(next_block.header, next_block.target);
        block_state.current_block = Some(next_block);
    }
    if block_state.current_block.is_none() {
        return Ok(());
    }
    let mut work = block_state.current_work;
    drop(block_state); // release lock
    let nonce = tokio::task::spawn_blocking({
        let server = Arc::clone(&server);
        move || {
            let mut miner = server.miner.lock().unwrap();
            if !miner.has_nonces_left(&work) {
                eprintln!("Exhaustively searched nonces, getblocktemplate too slow!");
                return Ok(None);
            }
            miner.find_nonce(&work)
        }
    })
    .await
    .unwrap()?;
    let mut block_state = server.block_state.lock().await;
    if let Some(nonce) = nonce {
        work.set_nonce(nonce);
        println!("Block hash below target!");
        if let Some(mut block) = block_state.current_block.take() {
            block.header = *work.header();
            if let Err(err) = submit_block(&server, &block).await {
                println!("submit_block error: {:?}", err);
            }
        } else {
            eprintln!("Found nonce but no block!");
        }
    }
    block_state.current_work.nonce_idx += 1;
    server
        .metrics_nonces
        .fetch_add(server.metrics_nonces_per_call, Ordering::AcqRel);
    let mut timestamp = server.metrics_timestamp.lock().await;
    let elapsed = match SystemTime::now().duration_since(*timestamp) {
        Ok(elapsed) => elapsed,
        Err(err) => {
            eprintln!("Elapsed time error: {}", err);
            return Ok(());
        }
    };
    if elapsed.as_secs() > 10 {
        let num_nonces = server.metrics_nonces.load(Ordering::Acquire);
        let hashrate = num_nonces as f64 / elapsed.as_secs_f64();
        println!("Hashrate: {:.3} MHash/s", hashrate / 1_000_000.0);
        server.metrics_nonces.store(0, Ordering::Release);
        *timestamp = SystemTime::now();
    }
    Ok(())
}

async fn submit_block(server: &Server, block: &Block) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(Deserialize)]
    struct SubmitBlockResponse {
        result: Option<String>,
    }
    let mut serialized_block = block.header.to_vec();
    encode_compact_size(&mut serialized_block, 0)?;
    encode_compact_size(&mut serialized_block, block.txs.len())?;
    for tx in &block.txs {
        serialized_block.extend_from_slice(&tx);
    }
    let response = init_request(server)
        .body(format!(
            r#"{{"method":"submitblock","params":[{:?}]}}"#,
            hex::encode(&serialized_block)
        ))
        .send()
        .await?;
    let response: SubmitBlockResponse = serde_json::from_str(&response.text().await?)?;
    match response.result {
        None => println!("BLOCK ACCEPTED!"),
        Some(reason) => println!("REJECTED BLOCK: {}", reason),
    }
    Ok(())
}
