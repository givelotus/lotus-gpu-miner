use std::{sync::Arc, time::Duration};

use lotus_miner_lib::{ConfigSettings, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: ConfigSettings = ConfigSettings::load(true)?;
    let report_hashrate_interval = Duration::from_secs(10);
    let server = Arc::new(Server::from_config(config, report_hashrate_interval));
    tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            loop {
                tokio::time::sleep(report_hashrate_interval).await;
                if let Some(hashrate) = server.log().hashrates().last() {
                    println!("{}", hashrate);
                }
            }
        }
    });
    server.run().await?;

    Ok(())
}
