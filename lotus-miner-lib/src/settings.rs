use std::io::Write;

use clap::{crate_authors, crate_description, crate_version, load_yaml, App};
use config::{Config, ConfigError, File};
use serde::Deserialize;

pub const DEFAULT_URL: &str = "http://127.0.0.1:10604";
pub const DEFAULT_USER: &str = "lotus";
pub const DEFAULT_PASSWORD: &str = "lotus";
pub const DEFAULT_RPC_POLL_INTERVAL: i64 = 3;
pub const FOLDER_DIR: &str = ".lotus-miner";
pub const DEFAULT_KERNEL_SIZE: i64 = 21;
pub const DEFAULT_GPU_INDEX: i64 = 0;

#[derive(Debug, Deserialize)]
pub struct ConfigSettings {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub rpc_poll_interval: i64,
    pub mine_to_address: String,
    pub kernel_size: i64,
    pub gpu_index: i64,
}

const DEFAULT_CONFIG_FILE_CONTENT: &str = r#"mine_to_address = ""
rpc_url = "http://127.0.0.1:10604"
rpc_poll_interval = 3
rpc_user = "lotus"
rpc_password = "lotus"
gpu_index = 0
kernel_size = 23
"#;

impl ConfigSettings {
    pub fn load(expect_mine_to_address: bool) -> Result<Self, ConfigError> {
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
        let default_config = home_dir;
        let default_config_folder = default_config.join(FOLDER_DIR);
        let default_config_toml = default_config_folder.join("config.toml");
        let default_config = default_config_folder.join("config");
        let default_config_str = default_config.to_str().unwrap();
        let config_path = match matches.value_of("config") {
            Some(config_path) => config_path,
            None => {
                if !default_config_toml.exists() {
                    if let Err(err) = std::fs::create_dir_all(&default_config_folder) {
                        eprintln!(
                            "Error: Couldn't create default config folder {}: {}",
                            default_config_folder.to_string_lossy(),
                            err
                        );
                    }
                    match std::fs::File::create(&default_config_toml) {
                        Ok(mut file) => {
                            if let Err(err) = file.write_all(DEFAULT_CONFIG_FILE_CONTENT.as_bytes())
                            {
                                eprintln!(
                                    "Error: Couldn't write default config toml file {}: {}",
                                    default_config_toml.to_string_lossy(),
                                    err
                                );
                            }
                        }
                        Err(err) => {
                            eprintln!(
                                "Error: Couldn't create default config toml file {}: {}",
                                default_config_toml.to_string_lossy(),
                                err
                            );
                        }
                    };
                }
                default_config_str
            }
        };
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
        if expect_mine_to_address
            && s.get_str("mine_to_address")
                .map(|mine_to_address| mine_to_address.is_empty())
                .unwrap_or(true)
        {
            return Err(ConfigError::Message(format!(
                "Must set mine_to_address config option. You can find it in {}.toml",
                std::fs::canonicalize(&config_path)
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|_| config_path.to_string())
            )));
        }

        // Set the bitcoin network
        if let Some(kernel_size) = matches.value_of("kernel_size") {
            s.set("kernel_size", kernel_size.parse::<i64>().unwrap())?;
        }

        // Set the GPU index
        if let Some(gpu_index) = matches.value_of("gpu_index") {
            s.set("gpu_index", gpu_index.parse::<i64>().unwrap())?;
        }

        s.try_into()
    }
}
