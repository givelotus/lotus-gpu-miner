use ocl::Kernel;

use crate::sha256::SHA256_K;

#[allow(non_snake_case)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Precalc {
    cty_a: u32,
    cty_b: u32,
    cty_c: u32,
    cty_d: u32,
    cty_e: u32,
    cty_f: u32,
    cty_g: u32,
    cty_h: u32,
    ctx_a: u32,
    ctx_b: u32,
    ctx_c: u32,
    ctx_d: u32,
    ctx_e: u32,
    ctx_f: u32,
    ctx_g: u32,
    ctx_h: u32,
    merkle: u32,
    ntime: u32,
    nbits: u32,
    nonce: u32,
    fW0: u32,
    fW1: u32,
    fW2: u32,
    fW3: u32,
    fW15: u32,
    fW01r: u32,
    fcty_e: u32,
    fcty_e2: u32,
    W16: u32,
    W17: u32,
    W2: u32,
    PreVal4: u32,
    T1: u32,
    C1addK5: u32,
    D1A: u32,
    W2A: u32,
    W17_2: u32,
    PreVal4addT1: u32,
    T1substate0: u32,
    PreVal4_2: u32,
    PreVal0: u32,
    PreW18: u32,
    PreW19: u32,
    PreW31: u32,
    PreW32: u32,

    /* For diakgcn */
    B1addK6: u32,
    PreVal0addK7: u32,
    W16addK16: u32,
    W17addK17: u32,
    zeroA: u32,
    zeroB: u32,
    oneA: u32,
    twoA: u32,
    threeA: u32,
    fourA: u32,
    fiveA: u32,
    sixA: u32,
    sevenA: u32,
}

#[allow(non_snake_case)]
fn R(a: u32, b: u32, c: u32, d: &mut u32, e: u32, f: u32, g: u32, h: &mut u32, w: u32, k: u32) {
    *h = h
        .wrapping_add(e.rotate_left(26) ^ e.rotate_left(21) ^ e.rotate_left(7))
        .wrapping_add(g ^ (e & (f ^ g)))
        .wrapping_add(k)
        .wrapping_add(w);
    *d = d.wrapping_add(*h);
    *h = h
        .wrapping_add(a.rotate_left(30) ^ a.rotate_left(19) ^ a.rotate_left(10))
        .wrapping_add((a & b) | (c & (a | b)));
}

#[allow(non_snake_case)]
pub fn precalc_hash(midstate: &[u32; 8], data: &[u32]) -> Precalc {
    let mut blk = Precalc::default();
    let A = midstate[0];
    let mut B = midstate[1];
    let mut C = midstate[2];
    let mut D = midstate[3];
    let E = midstate[4];
    let mut F = midstate[5];
    let mut G = midstate[6];
    let mut H = midstate[7];

    R(A, B, C, &mut D, E, F, G, &mut H, data[0], SHA256_K[0]);
    R(H, A, B, &mut C, D, E, F, &mut G, data[1], SHA256_K[1]);
    R(G, H, A, &mut B, C, D, E, &mut F, data[2], SHA256_K[2]);

    blk.cty_a = A;
    blk.cty_b = B;
    blk.cty_c = C;
    blk.cty_d = D;

    blk.D1A = D.wrapping_add(0xb956c25b);

    blk.cty_e = E;
    blk.cty_f = F;
    blk.cty_g = G;
    blk.cty_h = H;

    blk.ctx_a = midstate[0];
    blk.ctx_b = midstate[1];
    blk.ctx_c = midstate[2];
    blk.ctx_d = midstate[3];
    blk.ctx_e = midstate[4];
    blk.ctx_f = midstate[5];
    blk.ctx_g = midstate[6];
    blk.ctx_h = midstate[7];

    blk.merkle = data[0];
    blk.ntime = data[1];
    blk.nbits = data[2];

    blk.fW0 =
        data[0].wrapping_add(data[1].rotate_right(7) ^ data[1].rotate_right(18) ^ (data[1] >> 3));
    blk.W16 = blk.fW0;
    blk.fW1 = data[1]
        .wrapping_add(data[2].rotate_right(7) ^ data[2].rotate_right(18) ^ (data[2] >> 3))
        .wrapping_add(0x01100000);
    blk.W17 = blk.fW1;
    blk.fcty_e = blk
        .ctx_e
        .wrapping_add(B.rotate_right(6) ^ B.rotate_right(11) ^ B.rotate_right(25))
        .wrapping_add(D ^ (B & (C ^ D)))
        .wrapping_add(0xe9b5dba5);
    blk.PreVal4 = blk.fcty_e;
    blk.fcty_e2 = (F.rotate_right(2) ^ F.rotate_right(13) ^ F.rotate_right(22))
        .wrapping_add((F & G) | (H & (F | G)));
    blk.T1 = blk.fcty_e2;
    blk.PreVal4_2 = blk.PreVal4.wrapping_add(blk.T1);
    blk.PreVal0 = blk.PreVal4.wrapping_add(blk.ctx_a);
    blk.PreW31 = 0x00000280u32
        .wrapping_add(blk.W16.rotate_right(7) ^ blk.W16.rotate_right(18) ^ (blk.W16 >> 3));
    blk.PreW32 = blk
        .W16
        .wrapping_add(blk.W17.rotate_right(7) ^ blk.W17.rotate_right(18) ^ (blk.W17 >> 3));
    blk.PreW18 =
        data[2].wrapping_add(blk.W16.rotate_right(17) ^ blk.W16.rotate_right(19) ^ (blk.W16 >> 10));
    blk.PreW19 = 0x11002000u32
        .wrapping_add(blk.W17.rotate_right(17) ^ blk.W17.rotate_right(19) ^ (blk.W17 >> 10));

    blk.W2 = data[2];

    blk.W2A = blk
        .W2
        .wrapping_add(blk.W16.rotate_right(19) ^ blk.W16.rotate_right(17) ^ (blk.W16 >> 10));
    blk.W17_2 = 0x11002000u32
        .wrapping_add(blk.W17.rotate_right(19) ^ blk.W17.rotate_right(17) ^ (blk.W17 >> 10));

    blk.fW2 =
        data[2].wrapping_add(blk.fW0.rotate_right(17) ^ blk.fW0.rotate_right(19) ^ (blk.fW0 >> 10));
    blk.fW3 = 0x11002000u32
        .wrapping_add(blk.fW1.rotate_right(17) ^ blk.fW1.rotate_right(19) ^ (blk.fW1 >> 10));
    blk.fW15 = 0x00000280u32
        .wrapping_add(blk.fW0.rotate_right(7) ^ blk.fW0.rotate_right(18) ^ (blk.fW0 >> 3));
    blk.fW01r = blk
        .fW0
        .wrapping_add(blk.fW1.rotate_right(7) ^ blk.fW1.rotate_right(18) ^ (blk.fW1 >> 3));

    blk.PreVal4addT1 = blk.PreVal4.wrapping_add(blk.T1);
    blk.T1substate0 = blk.ctx_a.wrapping_sub(blk.T1);

    blk.C1addK5 = blk.cty_c.wrapping_add(SHA256_K[5]);
    blk.B1addK6 = blk.cty_b.wrapping_add(SHA256_K[6]);
    blk.PreVal0addK7 = blk.PreVal0.wrapping_add(SHA256_K[7]);
    blk.W16addK16 = blk.W16.wrapping_add(SHA256_K[16]);
    blk.W17addK17 = blk.W17.wrapping_add(SHA256_K[17]);

    blk.zeroA = blk.ctx_a.wrapping_add(0x98c7e2a2);
    blk.zeroB = blk.ctx_a.wrapping_add(0xfc08884d);
    blk.oneA = blk.ctx_b.wrapping_add(0x90bb1e3c);
    blk.twoA = blk.ctx_c.wrapping_add(0x50c6645b);
    blk.threeA = blk.ctx_d.wrapping_add(0x3ac42e24);
    blk.fourA = blk.ctx_e.wrapping_add(SHA256_K[4]);
    blk.fiveA = blk.ctx_f.wrapping_add(SHA256_K[5]);
    blk.sixA = blk.ctx_g.wrapping_add(SHA256_K[6]);
    blk.sevenA = blk.ctx_h.wrapping_add(SHA256_K[7]);
    return blk;
}

impl Precalc {
    pub fn set_kernel_args(&self, kernel: &mut Kernel) -> ocl::Result<()> {
        kernel.set_arg("state0", self.ctx_a)?;
        kernel.set_arg("state1", self.ctx_b)?;
        kernel.set_arg("state2", self.ctx_c)?;
        kernel.set_arg("state3", self.ctx_d)?;
        kernel.set_arg("state4", self.ctx_e)?;
        kernel.set_arg("state5", self.ctx_f)?;
        kernel.set_arg("state6", self.ctx_g)?;
        kernel.set_arg("state7", self.ctx_h)?;

        kernel.set_arg("b1", self.cty_b)?;
        kernel.set_arg("c1", self.cty_c)?;

        kernel.set_arg("f1", self.cty_f)?;
        kernel.set_arg("g1", self.cty_g)?;
        kernel.set_arg("h1", self.cty_h)?;

        kernel.set_arg("fw0", self.fW0)?;
        kernel.set_arg("fw1", self.fW1)?;
        kernel.set_arg("fw2", self.fW2)?;
        kernel.set_arg("fw3", self.fW3)?;
        kernel.set_arg("fw15", self.fW15)?;
        kernel.set_arg("fw01r", self.fW01r)?;

        kernel.set_arg("D1A", self.D1A)?;
        kernel.set_arg("C1addK5", self.C1addK5)?;
        kernel.set_arg("B1addK6", self.B1addK6)?;
        kernel.set_arg("W16addK16", self.W16addK16)?;
        kernel.set_arg("W17addK17", self.W17addK17)?;
        kernel.set_arg("PreVal4addT1", self.PreVal4addT1)?;
        kernel.set_arg("Preval0", self.PreVal0)?;
        Ok(())
    }
}
