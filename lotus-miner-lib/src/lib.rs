mod block;
mod miner;
mod settings;
mod sha256;

pub use settings::ConfigSettings;
pub use miner::Miner;

use std::{
    convert::TryInto,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use block::{create_block, Block, GetRawUnsolvedBlockResponse};
use miner::{MiningSettings, Work};
use rand::{Rng, SeedableRng};
use reqwest::RequestBuilder;
use serde::Deserialize;
use tokio::sync::Mutex;

pub struct Server {
    client: reqwest::Client,
    miner: std::sync::Mutex<Miner>,
    node_settings: Mutex<NodeSettings>,
    block_state: Mutex<BlockState>,
    rng: Mutex<rand::rngs::StdRng>,
    metrics_timestamp: Mutex<SystemTime>,
    metrics_nonces: AtomicU64,
    metrics_nonces_per_call: u64,
}

pub struct NodeSettings {
    pub bitcoind_url: String,
    pub bitcoind_user: String,
    pub bitcoind_password: String,
    pub rpc_poll_interval: u64,
    pub miner_addr: String,
}

struct BlockState {
    current_work: Work,
    current_block: Option<Block>,
    next_block: Option<Block>,
    extra_nonce: u64,
}

type ServerRef = Arc<Server>;

impl Server {
    pub fn from_config(config: ConfigSettings) -> Self {
        let mining_settings = MiningSettings {
            local_work_size: 256,
            inner_iter_size: 16,
            kernel_size: 1 << config.kernel_size,
            kernel_name: "lotus_og".to_string(),
            sleep: 0,
            gpu_indices: vec![config.gpu_index as usize],
        };
        let miner = Miner::setup(mining_settings.clone()).unwrap();
        Server {
            metrics_nonces_per_call: miner.num_nonces_per_search(),
            miner: std::sync::Mutex::new(miner),
            client: reqwest::Client::new(),
            node_settings: Mutex::new(NodeSettings {
                bitcoind_url: config.rpc_url.clone(),
                bitcoind_user: config.rpc_user.clone(),
                bitcoind_password: config.rpc_password.clone(),
                rpc_poll_interval: config.rpc_poll_interval.try_into().unwrap(),
                miner_addr: config.mine_to_address.clone(),
            }),
            block_state: Mutex::new(BlockState {
                current_work: Work::default(),
                current_block: None,
                next_block: None,
                extra_nonce: 0,
            }),
            rng: Mutex::new(rand::rngs::StdRng::from_entropy()),
            metrics_timestamp: Mutex::new(SystemTime::now()),
            metrics_nonces: AtomicU64::new(0),
        }
    }

    pub async fn run(self: ServerRef) -> Result<(), Box<dyn std::error::Error>> {
        let t1 = tokio::spawn({
            let server = Arc::clone(&self);
            async move {
                loop {
                    if let Err(err) = update_next_block(&server).await {
                        eprintln!("update_next_block error: {:?}", err);
                    }
                    tokio::time::sleep(Duration::from_secs(
                        server.node_settings.lock().await.rpc_poll_interval,
                    ))
                    .await;
                }
            }
        });
        let t2 = tokio::spawn({
            let server = Arc::clone(&self);
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
}

async fn init_request(server: &Server) -> RequestBuilder {
    let node_settings = server.node_settings.lock().await;
    server.client.post(&node_settings.bitcoind_url).basic_auth(
        &node_settings.bitcoind_user,
        Some(&node_settings.bitcoind_password),
    )
}

fn display_hash(hash: &[u8]) -> String {
    let mut hash = hash.to_vec();
    hash.reverse();
    hex::encode(&hash)
}

async fn update_next_block(server: &Server) -> Result<(), Box<dyn std::error::Error>> {
    let response = init_request(&server)
        .await
        .body(format!(
            r#"{{"method":"getrawunsolvedblock","params":["{}"]}}"#,
            server.node_settings.lock().await.miner_addr
        ))
        .send()
        .await?;
    let response = response.text().await?;
    let response: GetRawUnsolvedBlockResponse = serde_json::from_str(&response)?;
    let mut block_state = server.block_state.lock().await;
    let block = create_block(&response.result);
    if let Some(current_block) = &block_state.current_block {
        if current_block.prev_hash() != block.prev_hash() {
            println!(
                "Switched to new chain tip: {}",
                display_hash(&block.prev_hash())
            );
        }
    } else {
        println!(
            "Started mining on chain tip: {}",
            display_hash(&block.prev_hash())
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
    let big_nonce = server.rng.lock().await.gen();
    work.set_big_nonce(big_nonce);
    drop(block_state); // release lock
    let nonce = tokio::task::spawn_blocking({
        let server = Arc::clone(&server);
        move || {
            let mut miner = server.miner.lock().unwrap();
            if !miner.has_nonces_left(&work) {
                eprintln!(
                    "Error: Exhaustively searched nonces. This could be fixed by lowering \
                           rpc_poll_interval."
                );
                return Ok(None);
            }
            miner.find_nonce(&work)
        }
    })
    .await
    .unwrap()?;
    let mut block_state = server.block_state.lock().await;
    if let Some(nonce) = nonce {
        work.set_big_nonce(nonce);
        println!("Block hash below target with nonce: {}", nonce);
        if let Some(mut block) = block_state.current_block.take() {
            block.header = *work.header();
            if let Err(err) = submit_block(&server, &block).await {
                println!(
                    "submit_block error: {:?}. This could be a connection issue.",
                    err
                );
            }
        } else {
            eprintln!("BUG: Found nonce but no block! Contact the developers.");
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
            eprintln!("BUG: Elapsed time error: {}. Contact the developers.", err);
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
    serialized_block.extend_from_slice(&block.body);
    let response = init_request(server)
        .await
        .body(format!(
            r#"{{"method":"submitblock","params":[{:?}]}}"#,
            hex::encode(&serialized_block)
        ))
        .send()
        .await?;
    let response: SubmitBlockResponse = serde_json::from_str(&response.text().await?)?;
    match response.result {
        None => println!("BLOCK ACCEPTED!"),
        Some(reason) => {
            println!("REJECTED BLOCK: {}", reason);
            if reason == "inconclusive" {
                println!(
                    "This is an orphan race; might be fixed by lowering rpc_poll_interval or \
                          updating to the newest lotus-gpu-miner."
                );
            } else {
                println!(
                    "Something is misconfigured; make sure you run the latest \
                          lotusd/Lotus-QT and lotus-gpu-miner."
                );
            }
        }
    }
    Ok(())
}