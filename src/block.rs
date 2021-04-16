use std::{convert::TryInto, io::Write};

use bitcoincash_addr::{Address, HashType};
use hex_literal::hex;
use serde::Deserialize;

use crate::sha256::sha256d;

pub struct Block {
    pub header: [u8; 80],
    pub tx_hashes: Vec<[u8; 32]>,
    pub txs: Vec<Vec<u8>>,
    pub target: [u8; 32],
}

#[derive(Deserialize, Debug, Clone)]
pub struct GetBlockTemplateResponse {
    pub result: BlockTemplate,
    error: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BlockTemplate {
    pub version: u32,
    pub previousblockhash: String,
    pub transactions: Vec<BlockTemplateTx>,
    pub coinbasetxn: BlockTemplateCoinbaseTxn,
    pub coinbasevalue: u64,
    pub target: String,
    pub mintime: u32,
    pub curtime: u32,
    pub bits: String,
    pub height: i64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BlockTemplateTx {
    hash: String,
    data: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BlockTemplateCoinbaseTxn {
    minerfund: BlockTemplateMinerfund,
}

#[derive(Deserialize, Debug, Clone)]
struct BlockTemplateMinerfund {
    outputs: Vec<BlockTemplateMinerfundOutput>,
}

#[derive(Deserialize, Debug, Clone)]
struct BlockTemplateMinerfundOutput {
    value: u64,
    #[serde(rename = "scriptPubKey")]
    script_pubkey: String,
}

pub fn encode_compact_size(mut write: impl Write, size: usize) -> std::io::Result<usize> {
    if size < 0xfd {
        write.write_all(&[size as u8])?;
        Ok(1)
    } else if size <= 0xffff {
        write.write_all(&[0xfd])?;
        write.write_all(&(size as u16).to_le_bytes())?;
        Ok(3)
    } else if size <= 0xffff_ffff {
        write.write_all(&[0xfe])?;
        write.write_all(&(size as u32).to_le_bytes())?;
        Ok(5)
    } else {
        write.write_all(&[0xff])?;
        write.write_all(&(size as u64).to_le_bytes())?;
        Ok(9)
    }
}

fn encode_script_num(num: i64) -> Vec<u8> {
    if num == 0 {
        return vec![0];
    }
    if num >= -1 && num <= 16 {
        return vec![(num + 0x50) as u8];
    }
    let mut abs_val = num.abs();
    let mut result = vec![];
    while abs_val > 0 {
        result.push((abs_val & 0xff) as u8);
        abs_val >>= 8;
    }
    if result[result.len() - 1] & 0x80 != 0 {
        result.push(if num < 0 { 0x80 } else { 0 });
    } else if num < 0 {
        let len = result.len();
        result[len - 1] |= 0x80;
    }
    result.insert(0, result.len() as u8);
    result
}

fn get_merkle_root(mut hashes: Vec<[u8; 32]>) -> [u8; 32] {
    while hashes.len() > 1 {
        let mut new_hashes = Vec::new();
        for i in (0..hashes.len()).step_by(2) {
            let j = (i + 1).min(hashes.len() - 1);
            let mut data = [0; 64];
            data[..32].copy_from_slice(&hashes[i]);
            data[32..].copy_from_slice(&hashes[j]);
            new_hashes.push(sha256d(&data));
        }
        hashes = new_hashes;
    }
    return hashes[0];
}

fn create_coinbase(
    miner_addr: &Address,
    block_template: &BlockTemplate,
    extra_nonce: u64,
) -> Vec<u8> {
    let mut script_sig = b"\x05logos".to_vec();
    script_sig.extend(encode_script_num(block_template.height));
    script_sig.extend(vec![0; 10]);
    script_sig.extend(b"Lotus");
    script_sig.extend(&extra_nonce.to_le_bytes());
    let mut tx = Vec::new();
    tx.extend(&hex!("01000000")); // version
    tx.extend(&hex!("01")); // num inputs
    tx.extend(vec![0; 32]); // outpoint tx hash
    tx.extend(&hex!("ffffffff")); // outpoint output idx
    tx.push(script_sig.len() as u8); // script sig
    tx.extend(script_sig);
    tx.extend(&hex!("ffffffff")); // sequence no
    let mut miner_reward = block_template.coinbasevalue;
    let mut outputs = Vec::new();
    let minerfund = &block_template.coinbasetxn.minerfund;
    for output in &minerfund.outputs {
        miner_reward -= output.value;
        outputs.extend(&output.value.to_le_bytes());
        let script_pubkey = hex::decode(&output.script_pubkey).unwrap();
        outputs.push(script_pubkey.len() as u8);
        outputs.extend(script_pubkey);
    }
    encode_compact_size(&mut tx, minerfund.outputs.len() + 1).unwrap();
    let mut script_pubkey: Vec<u8> = Vec::new();
    match miner_addr.hash_type {
        HashType::Key => {
            script_pubkey.extend(&hex!("76a914"));
            script_pubkey.extend(&miner_addr.body);
            script_pubkey.extend(&hex!("88ac"));
        }
        HashType::Script => {
            script_pubkey.extend(&hex!("a914"));
            script_pubkey.extend(&miner_addr.body);
            script_pubkey.extend(&hex!("87"));
        }
    }
    tx.extend(&miner_reward.to_le_bytes());
    tx.push(script_pubkey.len() as u8);
    tx.extend(script_pubkey);
    tx.extend(outputs);
    tx.extend(&hex!("00000000")); // locktime
    tx
}

pub fn create_block(
    miner_addr: &Address,
    block_template: &BlockTemplate,
    extra_nonce: u64,
) -> Block {
    let coinbase = create_coinbase(miner_addr, block_template, extra_nonce);
    let mut tx_hashes = block_template
        .transactions
        .iter()
        .map(|tx| {
            let mut hash: [u8; 32] = hex::decode(&tx.hash).unwrap().try_into().unwrap();
            hash.reverse();
            hash
        })
        .collect::<Vec<_>>();
    let mut txs = block_template
        .transactions
        .iter()
        .map(|tx| hex::decode(&tx.data).unwrap())
        .collect::<Vec<_>>();
    tx_hashes.insert(0, sha256d(&coinbase));
    txs.insert(0, coinbase);
    let merkle_root = get_merkle_root(tx_hashes.clone());
    let mut header = Vec::with_capacity(80);
    header.extend(&block_template.version.to_le_bytes());
    header.extend(
        hex::decode(&block_template.previousblockhash)
            .unwrap()
            .iter()
            .rev()
            .cloned(),
    );
    header.extend(&merkle_root);
    header.extend(&block_template.curtime.to_le_bytes());
    header.extend(
        hex::decode(&block_template.bits)
            .unwrap()
            .iter()
            .rev()
            .cloned(),
    );
    header.extend(&[0; 4]);
    let mut target: [u8; 32] = hex::decode(&block_template.target)
        .unwrap()
        .try_into()
        .unwrap();
    target.reverse();
    Block {
        header: header.try_into().unwrap(),
        tx_hashes,
        txs,
        target,
    }
}
