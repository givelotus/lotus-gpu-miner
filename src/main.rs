use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use bitcoincash_addr::Address;
use block::{create_block, encode_compact_size, Block, GetBlockTemplateResponse};
use miner::{Miner, MiningSettings, Work};
use precalc::Precalc;
use reqwest::RequestBuilder;
use serde::Deserialize;
use tokio::sync::Mutex;

mod block;
mod miner;
mod precalc;
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
    current_precalc: Precalc,
    next_block: Option<Block>,
    extra_nonce: u64,
}

type ServerRef = Arc<Server>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mining_settings = MiningSettings {
        local_work_size: 256,
        kernel_size: 1 << 22,
        kernel_name: "poclbm120327".to_string(),
        sleep: 0,
    };
    let miner = Miner::setup(mining_settings.clone()).unwrap();
    let server = Arc::new(Server {
        metrics_nonces_per_call: miner.num_nonces_per_search(),
        miner: std::sync::Mutex::new(miner),
        client: reqwest::Client::new(),
        bitcoind_url: "http://127.0.0.1:7632".to_string(),
        bitcoind_user: "lotus".to_string(),
        bitcoind_password: "lotus".to_string(),
        miner_addr: Address::decode("bchtest:qrrmys82ksusqtyswkvz7fdwv2uvjccdty69ean7vs").unwrap(),
        block_state: Mutex::new(BlockState {
            current_work: Work::default(),
            current_block: None,
            current_precalc: Precalc::default(),
            next_block: None,
            extra_nonce: 0,
        }),
        metrics_timestamp: Mutex::new(SystemTime::now()),
        metrics_nonces: AtomicU64::new(0),
    });
    let t1 = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            loop {
                if let Err(err) = update_next_block(&server).await {
                    eprintln!("update_next_block error: {:?}", err);
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });
    let t2 = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            loop {
                if let Err(err) = mine_some_nonces(Arc::clone(&server)).await {
                    eprintln!("mine_some_nonces error: {:?}", err);
                }
                tokio::time::sleep(Duration::from_micros(3)).await;
            }
        }
    });
    t1.await?;
    t2.await?;

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
    let response: GetBlockTemplateResponse = serde_json::from_str(&response.text().await?)?;
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
        block_state.current_precalc = block_state.current_work.precalc();
        block_state.current_block = Some(next_block);
    }
    if block_state.current_block.is_none() {
        return Ok(());
    }
    let mut work = block_state.current_work;
    let precalc = block_state.current_precalc;
    drop(block_state); // release lock
    let nonce = tokio::task::spawn_blocking({
        let server = Arc::clone(&server);
        move || {
            let mut miner = server.miner.lock().unwrap();
            if !miner.has_nonces_left(&work) {
                eprintln!("Exhaustively searched nonces, getblocktemplate too slow!");
                return Ok(None);
            }
            miner.find_nonce(&work, &precalc)
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
