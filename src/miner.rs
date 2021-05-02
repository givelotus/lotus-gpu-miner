use ocl::{
    builders::{DeviceSpecifier, ProgramBuilder},
    Buffer, Device, Kernel, Platform, ProQue,
};
use sha2::Digest;
use std::convert::TryInto;

use crate::sha256::lotus_hash;

#[derive(Debug, Clone)]
pub struct MiningSettings {
    pub local_work_size: i32,
    pub kernel_size: u32,
    pub inner_iter_size: i32,
    pub kernel_name: String,
    pub sleep: u32,
    pub gpu_indices: Vec<usize>,
}

pub struct Miner {
    search_kernel: Kernel,
    header_buffer: Buffer<u32>,
    buffer: Buffer<u32>,
    settings: MiningSettings,
}

#[derive(Debug, Clone, Copy)]
pub struct Work {
    header: [u8; 160],
    target: [u8; 32],
    pub nonce_idx: u32,
}

impl Work {
    pub fn from_header(header: [u8; 160], target: [u8; 32]) -> Work {
        Work {
            header,
            target,
            nonce_idx: 0,
        }
    }

    pub fn set_nonce(&mut self, nonce: u32) {
        self.header[44..48].copy_from_slice(&nonce.to_le_bytes());
    }

    pub fn header(&self) -> &[u8; 160] {
        &self.header
    }
}

impl Default for Work {
    fn default() -> Self {
        Work {
            header: [0; 160],
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
            .cmplr_def("WORKSIZE", settings.local_work_size)
            .cmplr_def("ITERATIONS", settings.inner_iter_size);
        let platforms = Platform::list();
        println!("Platforms:");
        for (platform_idx, platform) in platforms.iter().enumerate() {
            println!(
                "{}: {}",
                platform_idx,
                platform.name().unwrap_or("<invalid platform>".to_string())
            );
            let devices = Device::list_all(platform)?;
            for (device_idx, device) in devices.iter().enumerate() {
                println!("- device {}: {}", device_idx, device.name()?);
            }
        }
        let pro_que = ProQue::builder()
            .device(DeviceSpecifier::WrappingIndices(
                settings.gpu_indices.clone(),
            ))
            .prog_bldr(builder)
            .dims(settings.kernel_size)
            .build()?;
        let search_kernel = pro_que
            .kernel_builder("search")
            .arg_named("offset", 0u32)
            .arg_named("partial_header", None::<&Buffer<u32>>)
            .arg_named("output", None::<&Buffer<u32>>)
            .build()?;
        let buffer = pro_que.buffer_builder::<u32>().len(0xff).build()?;
        let header_buffer = pro_que.buffer_builder::<u32>().len(0xff).build()?;
        Ok(Miner {
            search_kernel,
            buffer,
            header_buffer,
            settings,
        })
    }

    pub fn has_nonces_left(&self, work: &Work) -> bool {
        work.nonce_idx
            .checked_mul(self.settings.kernel_size)
            .is_some()
    }

    pub fn num_nonces_per_search(&self) -> u64 {
        self.settings.kernel_size as u64 * self.settings.inner_iter_size as u64
    }

    pub fn find_nonce(&mut self, work: &Work) -> ocl::Result<Option<u32>> {
        let base = match work
            .nonce_idx
            .checked_mul(self.num_nonces_per_search().try_into().unwrap())
        {
            Some(base) => base,
            None => {
                eprintln!("BUG: Nonce base overflow, skipping");
                return Ok(None);
            }
        };
        let mut partial_header = [0u8; 84];
        partial_header[..52].copy_from_slice(&work.header[..52]);
        partial_header[52..].copy_from_slice(&sha2::Sha256::digest(&work.header[52..]));
        let mut partial_header_ints = [0u32; 21];
        for (chunk, int) in partial_header.chunks(4).zip(partial_header_ints.iter_mut()) {
            *int = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        self.header_buffer.write(&partial_header_ints[..]).enq()?;
        self.search_kernel
            .set_arg("partial_header", &self.header_buffer)?;
        self.search_kernel.set_arg("output", &self.buffer)?;
        self.search_kernel.set_arg("offset", base)?;
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
                    header[44..48].copy_from_slice(&nonce.to_le_bytes());
                    let hash = lotus_hash(&header);
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
