mod block;
mod miner;
pub mod settings;
mod sha256;

use eyre::Result;
pub use miner::Miner;
pub use settings::ConfigSettings;

use std::{
    convert::TryInto,
    fmt::Display,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use block::{create_block, Block, GetRawUnsolvedBlockResponse};
use miner::{MiningSettings, Work};
use rand::{Rng, SeedableRng};
use reqwest::{RequestBuilder, StatusCode};
use serde::Deserialize;
use tokio::sync::{Mutex, MutexGuard};

pub struct Server {
    client: reqwest::Client,
    miner: std::sync::Mutex<Miner>,
    node_settings: Mutex<NodeSettings>,
    block_state: Mutex<BlockState>,
    rng: Mutex<rand::rngs::StdRng>,
    metrics_timestamp: Mutex<SystemTime>,
    metrics_nonces: AtomicU64,
    log: Log,
    report_hashrate_interval: Duration,
}

pub struct NodeSettings {
    pub bitcoind_url: String,
    pub bitcoind_user: String,
    pub bitcoind_password: String,
    pub rpc_poll_interval: u64,
    pub miner_addr: String,
}

pub struct Log {
    logs: std::sync::RwLock<Vec<LogEntry>>,
    hashrates: std::sync::RwLock<Vec<HashrateEntry>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSeverity {
    Info,
    Warn,
    Error,
    Bug,
}

pub struct LogEntry {
    pub msg: String,
    pub severity: LogSeverity,
    pub timestamp: chrono::DateTime<chrono::Local>,
}

pub struct HashrateEntry {
    pub hashrate: f64,
    pub timestamp: chrono::DateTime<chrono::Local>,
}

struct BlockState {
    current_work: Work,
    current_block: Option<Block>,
    next_block: Option<Block>,
    extra_nonce: u64,
}

pub type ServerRef = Arc<Server>;

impl Server {
    pub fn from_config(config: ConfigSettings, report_hashrate_interval: Duration) -> Self {
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
            log: Log::new(),
            report_hashrate_interval,
        }
    }

    pub async fn run(self: ServerRef) -> Result<(), Box<dyn std::error::Error>> {
        let t1 = tokio::spawn({
            let server = Arc::clone(&self);
            async move {
                let log = server.log();
                loop {
                    if let Err(err) = update_next_block(&server).await {
                        log.error(format!("update_next_block error: {:?}", err));
                    }
                    let rpc_poll_interval = server.node_settings.lock().await.rpc_poll_interval;
                    tokio::time::sleep(Duration::from_secs(rpc_poll_interval)).await;
                }
            }
        });
        let t2 = tokio::spawn({
            let server = Arc::clone(&self);
            async move {
                let log = server.log();
                loop {
                    if let Err(err) = mine_some_nonces(Arc::clone(&server)).await {
                        log.error(format!("mine_some_nonces error: {:?}", err));
                    }
                    tokio::time::sleep(Duration::from_micros(3)).await;
                }
            }
        });
        t1.await?;
        t2.await?;
        Ok(())
    }

    pub async fn node_settings<'a>(&'a self) -> MutexGuard<'a, NodeSettings> {
        self.node_settings.lock().await
    }

    pub fn miner<'a>(&'a self) -> std::sync::MutexGuard<'a, Miner> {
        self.miner.lock().unwrap()
    }

    pub fn log(&self) -> &Log {
        &self.log
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
    let log = server.log();
    let response = init_request(&server)
        .await
        .body(format!(
            r#"{{"method":"getrawunsolvedblock","params":["{}"]}}"#,
            server.node_settings.lock().await.miner_addr
        ))
        .send()
        .await?;
    let status = response.status();
    let response_str = response.text().await?;
    let response: Result<GetRawUnsolvedBlockResponse, _> = serde_json::from_str(&response_str);
    let response = match response {
        Ok(response) => response,
        Err(_) => {
            log.error(format!(
                "getrawunsolvedblock failed ({}): {}",
                status, response_str
            ));
            if status == StatusCode::UNAUTHORIZED {
                log.error("It seems you specified the wrong username/password");
            }
            return Ok(());
        }
    };
    let mut block_state = server.block_state.lock().await;
    let unsolved_block = match response.result {
        Some(unsolved_block) => unsolved_block,
        None => {
            log.error(format!(
                "getrawunsolvedblock failed: {}",
                response.error.unwrap_or("unknown error".to_string())
            ));
            return Ok(());
        }
    };
    let block = create_block(&unsolved_block);
    if let Some(current_block) = &block_state.current_block {
        if current_block.prev_hash() != block.prev_hash() {
            log.info(format!(
                "Switched to new chain tip: {}",
                display_hash(&block.prev_hash())
            ));
        }
    } else {
        log.info(format!(
            "Started mining on chain tip: {}",
            display_hash(&block.prev_hash())
        ));
    }
    block_state.extra_nonce += 1;
    block_state.next_block = Some(block);
    Ok(())
}

async fn mine_some_nonces(server: ServerRef) -> Result<()> {
    let log = server.log();
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
    let (nonce, num_nonces_per_search) = tokio::task::spawn_blocking({
        let server = Arc::clone(&server);
        move || {
            let log = server.log();
            let mut miner = server.miner.lock().unwrap();
            if !miner.has_nonces_left(&work) {
                log.error(format!(
                    "Error: Exhaustively searched nonces. This could be fixed by lowering \
                           rpc_poll_interval."
                ));
                return Ok((None, 0));
            }
            miner
                .find_nonce(&work, server.log())
                .map(|nonce| (nonce, miner.num_nonces_per_search()))
        }
    })
    .await
    .unwrap()?;
    let mut block_state = server.block_state.lock().await;
    if let Some(nonce) = nonce {
        work.set_big_nonce(nonce);
        log.info(format!("Block hash below target with nonce: {}", nonce));
        if let Some(mut block) = block_state.current_block.take() {
            block.header = *work.header();
            if let Err(err) = submit_block(&server, &block).await {
                log.error(format!(
                    "submit_block error: {:?}. This could be a connection issue.",
                    err
                ));
            }
        } else {
            log.bug("BUG: Found nonce but no block! Contact the developers.");
        }
    }
    block_state.current_work.nonce_idx += 1;
    server
        .metrics_nonces
        .fetch_add(num_nonces_per_search, Ordering::AcqRel);
    let mut timestamp = server.metrics_timestamp.lock().await;
    let elapsed = match SystemTime::now().duration_since(*timestamp) {
        Ok(elapsed) => elapsed,
        Err(err) => {
            log.bug(format!(
                "BUG: Elapsed time error: {}. Contact the developers.",
                err
            ));
            return Ok(());
        }
    };
    if elapsed > server.report_hashrate_interval {
        let num_nonces = server.metrics_nonces.load(Ordering::Acquire);
        let hashrate = num_nonces as f64 / elapsed.as_secs_f64();
        log.report_hashrate(hashrate);
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
    let log = server.log();
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
        None => log.info("BLOCK ACCEPTED!"),
        Some(reason) => {
            log.error(format!("REJECTED BLOCK: {}", reason));
            if reason == "inconclusive" {
                log.warn(
                    "This is an orphan race; might be fixed by lowering rpc_poll_interval or \
                          updating to the newest lotus-gpu-miner.",
                );
            } else {
                log.error(
                    "Something is misconfigured; make sure you run the latest \
                          lotusd/Lotus-QT and lotus-gpu-miner.",
                );
            }
        }
    }
    Ok(())
}

impl Log {
    pub fn new() -> Self {
        Log {
            logs: std::sync::RwLock::new(Vec::new()),
            hashrates: std::sync::RwLock::new(Vec::new()),
        }
    }

    pub fn log(&self, entry: impl Into<LogEntry>) {
        let mut logs = self.logs.write().unwrap();
        let entry = entry.into();
        println!("{}", entry);
        logs.push(entry);
    }

    pub fn log_str(&self, msg: impl ToString, severity: LogSeverity) {
        self.log(LogEntry {
            msg: msg.to_string(),
            severity,
            timestamp: chrono::Local::now(),
        })
    }

    pub fn info(&self, msg: impl ToString) {
        self.log_str(msg, LogSeverity::Info)
    }

    pub fn warn(&self, msg: impl ToString) {
        self.log_str(msg, LogSeverity::Warn)
    }

    pub fn error(&self, msg: impl ToString) {
        self.log_str(msg, LogSeverity::Error)
    }

    pub fn bug(&self, msg: impl ToString) {
        self.log_str(msg, LogSeverity::Bug)
    }

    pub fn get_logs_and_clear(&self) -> Vec<LogEntry> {
        let mut logs = self.logs.write().unwrap();
        logs.drain(..).collect()
    }

    pub fn report_hashrate(&self, hashrate: f64) {
        let mut hashrates = self.hashrates.write().unwrap();
        hashrates.push(HashrateEntry {
            hashrate,
            timestamp: chrono::Local::now(),
        });
    }

    pub fn hashrates<'a>(&'a self) -> std::sync::RwLockReadGuard<'a, Vec<HashrateEntry>> {
        self.hashrates.read().unwrap()
    }
}

impl Display for LogEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{:?}] {}",
            self.timestamp.to_rfc3339(),
            self.severity,
            self.msg
        )
    }
}

impl Display for HashrateEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} Hashrate {:.3} MH/s",
            self.timestamp.to_rfc3339(),
            self.hashrate / 1_000_000.0
        )
    }
}
