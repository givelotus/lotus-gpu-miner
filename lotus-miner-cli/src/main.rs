use std::sync::Arc;

use lotus_miner_lib::{ConfigSettings, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: ConfigSettings = ConfigSettings::load(true)?;
    let server = Arc::new(Server::from_config(config));
    server.run().await?;

    Ok(())
}
