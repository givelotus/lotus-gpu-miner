use std::{convert::TryInto, u32};

use hex_literal::hex;
use ocl::{builders::ProgramBuilder, Buffer, Kernel, ProQue};

use crate::{
    precalc::{precalc_hash, Precalc},
    sha256::{sha256d, Sha256},
};

const SHA256_PADDING: [u8; 48] = hex!("800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000280");

#[derive(Debug, Clone)]
pub struct MiningSettings {
    pub local_work_size: i32,
    pub kernel_size: u32,
    pub kernel_name: String,
    pub sleep: u32,
}

pub struct Miner {
    search_kernel: Kernel,
    buffer: Buffer<u32>,
    settings: MiningSettings,
}

#[derive(Debug, Clone, Copy)]
pub struct Work {
    header: [u8; 80],
    data: [u8; 128],
    midstate: [u32; 8],
    target: [u8; 32],
    pub nonce_idx: u32,
}

impl Work {
    pub fn from_header(header: [u8; 80], target: [u8; 32]) -> Work {
        let mut data = [0; 128];
        data[..80].copy_from_slice(&header);
        data[80..].copy_from_slice(&SHA256_PADDING);
        let mut hash = Sha256::new();
        hash.update_prepad(&data[..64].try_into().unwrap());
        Work {
            header,
            data,
            midstate: hash.state(),
            target,
            nonce_idx: 0,
        }
    }

    pub fn precalc(&self) -> Precalc {
        let mut data_be = [0u32; 32];
        for (chunk, data) in self.data.chunks(4).zip(data_be.iter_mut()) {
            *data = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        precalc_hash(&self.midstate, &data_be[16..])
    }

    pub fn set_nonce(&mut self, nonce: u32) {
        self.header[76..].copy_from_slice(&nonce.to_le_bytes());
    }

    pub fn header(&self) -> &[u8; 80] {
        &self.header
    }
}

impl Default for Work {
    fn default() -> Self {
        Work {
            header: [0; 80],
            data: [0; 128],
            midstate: [0; 8],
            target: [0; 32],
            nonce_idx: 0,
        }
    }
}

impl Miner {
    pub fn setup(settings: MiningSettings) -> ocl::Result<Self> {
        let mut builder = ProgramBuilder::new();
        builder
            .src_file(format!("kernels/{}.cl", settings.kernel_name))
            .cmplr_def("WORKSIZE", settings.local_work_size);
        let pro_que = ProQue::builder()
            .prog_bldr(builder)
            .dims(settings.kernel_size)
            .build()?;
        let search_kernel = pro_que
            .kernel_builder("search")
            .arg_named("state0", 0)
            .arg_named("state1", 0)
            .arg_named("state2", 0)
            .arg_named("state3", 0)
            .arg_named("state4", 0)
            .arg_named("state5", 0)
            .arg_named("state6", 0)
            .arg_named("state7", 0)
            .arg_named("b1", 0)
            .arg_named("c1", 0)
            .arg_named("f1", 0)
            .arg_named("g1", 0)
            .arg_named("h1", 0)
            .arg_named("base", 0)
            .arg_named("fw0", 0)
            .arg_named("fw1", 0)
            .arg_named("fw2", 0)
            .arg_named("fw3", 0)
            .arg_named("fw15", 0)
            .arg_named("fw01r", 0)
            .arg_named("D1A", 0)
            .arg_named("C1addK5", 0)
            .arg_named("B1addK6", 0)
            .arg_named("W16addK16", 0)
            .arg_named("W17addK17", 0)
            .arg_named("PreVal4addT1", 0)
            .arg_named("Preval0", 0)
            .arg_named("output", None::<&Buffer<u32>>)
            .build()?;
        let buffer = pro_que.buffer_builder::<u32>().len(0xff).build()?;
        Ok(Miner {
            search_kernel,
            buffer,
            settings,
        })
    }

    pub fn has_nonces_left(&self, work: &Work) -> bool {
        work.nonce_idx
            .checked_mul(self.settings.kernel_size)
            .is_some()
    }

    pub fn num_nonces_per_search(&self) -> u64 {
        self.settings.kernel_size as u64
    }

    pub fn find_nonce(&mut self, work: &Work, precalc: &Precalc) -> ocl::Result<Option<u32>> {
        let base = match work.nonce_idx.checked_mul(self.settings.kernel_size) {
            Some(base) => base,
            None => {
                eprintln!("BUG: Nonce base overflow, skipping");
                return Ok(None);
            }
        };
        precalc.set_kernel_args(&mut self.search_kernel)?;
        self.search_kernel.set_arg("output", &self.buffer)?;
        self.search_kernel.set_arg("base", base)?;
        let mut vec = vec![0; self.buffer.len()];
        self.buffer.write(&vec).enq()?;
        let cmd = self
            .search_kernel
            .cmd()
            .local_work_size(self.settings.local_work_size);
        unsafe {
            cmd.enq()?;
        }
        self.buffer.read(&mut vec).enq()?;
        if vec[0x80] != 0 {
            let mut header = work.header;
            'nonce: for &nonce in &vec[..0x7f] {
                let nonce = nonce.swap_bytes();
                if nonce != 0 {
                    header[76..].copy_from_slice(&nonce.to_le_bytes());
                    let hash = sha256d(&header);
                    let mut candidate_hash = hash;
                    candidate_hash.reverse();
                    println!(
                        "Candidate: nonce={}, hash={}",
                        nonce,
                        hex::encode(&candidate_hash)
                    );
                    if hash.last() != Some(&0) {
                        eprintln!("BUG: found nonce's hash has no leading zero byte");
                    }
                    for (&h, &t) in hash.iter().zip(work.target.iter()).rev() {
                        if h > t {
                            continue 'nonce;
                        }
                        if t > h {
                            return Ok(Some(nonce));
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}
