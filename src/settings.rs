use clap::{crate_authors, crate_description, crate_version, load_yaml, App};
use config::{Config, ConfigError, File};
use serde::Deserialize;

const DEFAULT_URL: &str = "http://127.0.0.1:7632";
const DEFAULT_USER: &str = "lotus";
const DEFAULT_PASSWORD: &str = "lotus";
const DEFAULT_RPC_POLL_INTERVAL: i64 = 3;
const FOLDER_DIR: &str = ".lotus-miner";
const DEFAULT_KERNEL_SIZE: i64 = 21;
const DEFAULT_GPU_INDEX: i64 = 0;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub rpc_poll_interval: i64,
    pub mine_to_address: String,
    pub kernel_size: i64,
    pub gpu_index: i64,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut s = Config::new();

        // Set defaults
        let yaml = load_yaml!("cli.yaml");
        let matches = App::from_yaml(yaml)
            .about(crate_description!())
            .author(crate_authors!("\n"))
            .version(crate_version!())
            .get_matches();
        let home_dir = match dirs::home_dir() {
            Some(some) => some,
            None => return Err(ConfigError::Message("no home directory".to_string())),
        };
        s.set_default("rpc_url", DEFAULT_URL)?;
        s.set_default("rpc_poll_interval", DEFAULT_RPC_POLL_INTERVAL)?;
        s.set_default("rpc_user", DEFAULT_USER)?;
        s.set_default("rpc_password", DEFAULT_PASSWORD)?;
        s.set_default("kernel_size", DEFAULT_KERNEL_SIZE)?;
        s.set_default("gpu_index", DEFAULT_GPU_INDEX)?;

        // Load config from file
        let mut default_config = home_dir;
        default_config.push(format!("{}/config", FOLDER_DIR));
        let default_config_str = default_config.to_str().unwrap();
        let config_path = matches.value_of("config").unwrap_or(default_config_str);
        s.merge(File::with_name(config_path).required(false))?;

        // Set bind address from cmd line
        if let Some(rpc_url) = matches.value_of("rpc_url") {
            s.set("rpc_url", rpc_url)?;
        }

        // Set the bitcoin network
        if let Some(rpc_poll_interval) = matches.value_of("rpc_poll_interval") {
            s.set(
                "rpc_poll_interval",
                rpc_poll_interval.parse::<i64>().unwrap(),
            )?;
        }

        // Set bind address from cmd line
        if let Some(rpc_password) = matches.value_of("rpc_password") {
            s.set("rpc_password", rpc_password)?;
        }

        // Set the bitcoin network
        if let Some(rpc_user) = matches.value_of("rpc_user") {
            s.set("rpc_user", rpc_user)?;
        }

        // Set the bitcoin network
        if let Some(mine_to_address) = matches.value_of("mine_to_address") {
            s.set("mine_to_address", mine_to_address)?;
        }

        // Set the bitcoin network
        if let Some(kernel_size) = matches.value_of("kernel_size") {
            s.set("kernel_size", kernel_size.parse::<i64>().unwrap())?;
        }

        // Set the bitcoin network
        if let Some(gpu_index) = matches.value_of("gpu_index") {
            s.set("gpu_index", gpu_index.parse::<i64>().unwrap())?;
        }

        s.try_into()
    }
}
