use std::convert::TryInto;
use std::io::BufRead;

use miner::{Miner, MiningSettings, Work};

use crate::sha256::lotus_hash;

mod block;
mod miner;
mod sha256;

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
        kernel_size: 1 << 18,
        kernel_name: "lotus_og".to_string(),
        sleep: 0,
        gpu_indices: vec![1],
    };
    let mut miner = Miner::setup(mining_settings.clone()).unwrap();
    let mut found_nonces = Vec::new();

    let file = std::fs::File::open("headers.txt")?;
    let mut headers = std::io::BufReader::new(file).lines();
    let header_hex: String = headers.next().unwrap().unwrap();
    let header: [u8; 160] = hex::decode(&header_hex)?.try_into().unwrap();
    let mut big_nonce = 0u32;
    let n_bits = u32::from_le_bytes(header[32..36].try_into().unwrap());
    let target = n_bits_to_target(n_bits);
    println!("target: {}", hex::encode(&target));
    let mut work = Work::from_header(header, target);
    loop {
        work.set_big_nonce(big_nonce);
        while miner.has_nonces_left(&work) {
            if let Some(nonce) = miner.find_nonce(&work).unwrap() {
                let mut result_nonce = big_nonce as u64;
                result_nonce <<= 32;
                result_nonce |= nonce as u64;
                work.set_nonce(nonce);
                let mut solved_hash = lotus_hash(work.header());
                found_nonces.push(result_nonce);
                solved_hash.reverse();
                println!(
                    "{}, {}, {}",
                    hex::encode(&solved_hash),
                    result_nonce,
                    hex::encode(work.header())
                );
            }
            work.nonce_idx += 1;
        }
        big_nonce += 1;
        work.nonce_idx = 0;
    }
}
