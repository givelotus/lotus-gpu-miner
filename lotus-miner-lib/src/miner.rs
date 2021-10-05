use ocl::{
    builders::{DeviceSpecifier, ProgramBuilder},
    Buffer, Context, Device, Kernel, Platform, Queue,
};
use sha2::Digest;
use std::convert::TryInto;
use eyre::Result;
use thiserror::Error;

use crate::{sha256::lotus_hash, Log};

#[derive(Debug, Error)]
pub enum MinerError {
    #[error("Ocl error: {0:?}")]
    Ocl(ocl::Error)
}

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

impl From<ocl::Error> for MinerError {
    fn from(err: ocl::Error) -> Self {
        MinerError::Ocl(err)
    }
}

use self::MinerError::*;

impl Work {
    pub fn from_header(header: [u8; 160], target: [u8; 32]) -> Work {
        Work {
            header,
            target,
            nonce_idx: 0,
        }
    }

    pub fn set_big_nonce(&mut self, big_nonce: u64) {
        self.header[44..52].copy_from_slice(&big_nonce.to_le_bytes());
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
    pub fn setup(settings: MiningSettings) -> Result<Self> {
        let mut prog_builder = ProgramBuilder::new();
        prog_builder
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
            let devices = Device::list_all(platform).map_err(Ocl)?;
            for (device_idx, device) in devices.iter().enumerate() {
                println!("- device {}: {}", device_idx, device.name().map_err(Ocl)?);
            }
        }
        let mut platform_device = None;
        let mut gpu_index = 0;
        for cur_platform in platforms {
            if let Ok(devices) = Device::list_all(cur_platform.clone()) {
                for cur_device in devices {
                    if gpu_index == settings.gpu_indices[0] {
                        platform_device = Some((cur_platform, cur_device));
                    }
                    gpu_index += 1;
                }
            }
        }
        let (platform, device) = platform_device.expect("No such GPU");
        let ctx = Context::builder()
            .platform(platform.clone())
            .devices(DeviceSpecifier::Single(device.clone()))
            .build().map_err(Ocl)?;
        let queue = Queue::new(&ctx, device, None).map_err(Ocl)?;
        prog_builder.devices(DeviceSpecifier::Single(device.clone()));
        let program = prog_builder.build(&ctx).map_err(Ocl)?;
        let mut kernel_builder = Kernel::builder();
        kernel_builder
            .program(&program)
            .name("search")
            .queue(queue.clone());
        let buffer = Buffer::builder().len(0xff).queue(queue.clone()).build().map_err(Ocl)?;
        let header_buffer = Buffer::builder().len(0xff).queue(queue).build().map_err(Ocl)?;
        let search_kernel = kernel_builder
            .arg_named("offset", 0u32)
            .arg_named("partial_header", None::<&Buffer<u32>>)
            .arg_named("output", None::<&Buffer<u32>>)
            .build().map_err(Ocl)?;
        Ok(Miner {
            search_kernel,
            buffer,
            header_buffer,
            settings,
        })
    }

    pub fn list_device_names() -> Vec<String> {
        let platforms = Platform::list();
        let mut device_names = Vec::new();
        for platform in platforms.iter() {
            let platform_name = platform.name().unwrap_or("<invalid platform>".to_string());
            let devices = Device::list_all(platform).unwrap_or(vec![]);
            for device in devices.iter() {
                device_names.push(format!(
                    "{} - {}",
                    platform_name,
                    device.name().unwrap_or("<invalid device>".to_string())
                ));
            }
        }
        device_names
    }

    pub fn has_nonces_left(&self, work: &Work) -> bool {
        work.nonce_idx
            .checked_mul(self.settings.kernel_size)
            .is_some()
    }

    pub fn num_nonces_per_search(&self) -> u64 {
        self.settings.kernel_size as u64 * self.settings.inner_iter_size as u64
    }

    pub fn find_nonce(&mut self, work: &Work, log: &Log) -> Result<Option<u64>> {
        let base = match work
            .nonce_idx
            .checked_mul(self.num_nonces_per_search().try_into().unwrap())
        {
            Some(base) => base,
            None => {
                log.error(
                    "Error: Nonce base overflow, skipping. This could be fixed by lowering \
                           rpc_poll_interval.",
                );
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
        self.header_buffer.write(&partial_header_ints[..]).enq().map_err(Ocl)?;
        self.search_kernel
            .set_arg("partial_header", &self.header_buffer).map_err(Ocl)?;
        self.search_kernel.set_arg("output", &self.buffer).map_err(Ocl)?;
        self.search_kernel.set_arg("offset", base).map_err(Ocl)?;
        let mut vec = vec![0; self.buffer.len()];
        self.buffer.write(&vec).enq().map_err(Ocl)?;
        let cmd = self
            .search_kernel
            .cmd()
            .global_work_size(self.settings.kernel_size);
        unsafe {
            cmd.enq().map_err(Ocl)?;
        }
        self.buffer.read(&mut vec).enq().map_err(Ocl)?;
        if vec[0x80] != 0 {
            let mut header = work.header;
            'nonce: for &nonce in &vec[..0x7f] {
                let nonce = nonce.swap_bytes();
                if nonce != 0 {
                    header[44..48].copy_from_slice(&nonce.to_le_bytes());
                    let result_nonce = u64::from_le_bytes(header[44..52].try_into().unwrap());
                    let hash = lotus_hash(&header);
                    let mut candidate_hash = hash;
                    candidate_hash.reverse();
                    log.info(format!(
                        "Candidate: nonce={}, hash={}",
                        result_nonce,
                        hex::encode(&candidate_hash)
                    ));
                    if hash.last() != Some(&0) {
                        log.bug(
                            "BUG: found nonce's hash has no leading zero byte. Contact the \
                                   developers.",
                        );
                    }
                    for (&h, &t) in hash.iter().zip(work.target.iter()).rev() {
                        if h > t {
                            continue 'nonce;
                        }
                        if t > h {
                            return Ok(Some(result_nonce));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn set_intensity(&mut self, intensity: i32) {
        self.settings.kernel_size = 1 << intensity;
    }

    pub fn update_gpu_index(&mut self, gpu_index: i64) -> Result<()> {
        if self.settings.gpu_indices[0] == gpu_index as usize {
            return Ok(());
        }
        let mut settings = self.settings.clone();
        settings.gpu_indices = vec![gpu_index.try_into().unwrap()];
        *self = Miner::setup(settings)?;
        Ok(())
    }
}
