//! ChaCha8 CSPRNG (mirrors `random.c`): per-heap streams for free-list
//! encoding keys, guarded-object sampling, and page-start randomization.
//!
//! Self-contained (no rand crate — the allocator cannot depend on code that
//! allocates). Seeded from OS entropy where available, else from a mix of
//! clock, addresses and thread id; the seed path is documented per platform
//! so `secure` builds can state what they rest on.

use core::sync::atomic::{AtomicU64, Ordering};

/// A ChaCha8 stream. Not `Sync`: each heap owns one (no sharing, no locks).
pub struct Random {
    state: [u32; 16],
    out: [u32; 16],
    used: usize,
}

#[inline]
fn qround(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[a] = x[a].wrapping_add(x[b]);
    x[d] = (x[d] ^ x[a]).rotate_left(16);
    x[c] = x[c].wrapping_add(x[d]);
    x[b] = (x[b] ^ x[c]).rotate_left(12);
    x[a] = x[a].wrapping_add(x[b]);
    x[d] = (x[d] ^ x[a]).rotate_left(8);
    x[c] = x[c].wrapping_add(x[d]);
    x[b] = (x[b] ^ x[c]).rotate_left(7);
}

impl Random {
    /// Empty (unseeded) stream — call [`reseed`](Self::reseed) before use.
    pub const fn new() -> Random {
        Random {
            state: [0; 16],
            out: [0; 16],
            used: 16,
        }
    }

    /// Seed from `key` (32 bytes as 8 words) and a stream id.
    pub fn seed_from(&mut self, key: [u32; 8], stream: u64) {
        // "expand 32-byte k"
        self.state[0] = 0x6170_7865;
        self.state[1] = 0x3320_646e;
        self.state[2] = 0x7962_2d32;
        self.state[3] = 0x6b20_6574;
        self.state[4..12].copy_from_slice(&key);
        self.state[12] = 0; // counter
        self.state[13] = 0;
        self.state[14] = stream as u32;
        self.state[15] = (stream >> 32) as u32;
        self.used = 16;
    }

    /// Seed from OS entropy plus environmental mixing.
    pub fn reseed(&mut self) {
        let mut key = [0u32; 8];
        if !os_entropy(&mut key) {
            // Fallback mixing: clock, a stack address, a heap-ish address,
            // thread id, and a global counter (documented weaker path).
            static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
            let stack = &key as *const _ as usize as u64;
            let mut acc = crate::prim::clock_now()
                ^ stack.rotate_left(17)
                ^ (crate::prim::thread_id() as u64).rotate_left(33)
                ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
            for k in key.iter_mut() {
                // splitmix64 step
                acc = acc.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = acc;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                *k = ((z ^ (z >> 31)) & 0xFFFF_FFFF) as u32;
            }
        }
        let stream = crate::prim::thread_id() as u64;
        self.seed_from(key, stream);
    }

    fn refill(&mut self) {
        let mut x = self.state;
        for _ in 0..4 {
            // 8 rounds = 4 double-rounds
            qround(&mut x, 0, 4, 8, 12);
            qround(&mut x, 1, 5, 9, 13);
            qround(&mut x, 2, 6, 10, 14);
            qround(&mut x, 3, 7, 11, 15);
            qround(&mut x, 0, 5, 10, 15);
            qround(&mut x, 1, 6, 11, 12);
            qround(&mut x, 2, 7, 8, 13);
            qround(&mut x, 3, 4, 9, 14);
        }
        for (o, (xi, si)) in self.out.iter_mut().zip(x.iter().zip(self.state.iter())) {
            *o = xi.wrapping_add(*si);
        }
        // Bump the 64-bit counter.
        let (lo, carry) = self.state[12].overflowing_add(1);
        self.state[12] = lo;
        if carry {
            self.state[13] = self.state[13].wrapping_add(1);
        }
        self.used = 0;
    }

    /// Next 32 random bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        if self.used >= 16 {
            self.refill();
        }
        let v = self.out[self.used];
        self.used += 1;
        v
    }

    /// A full `usize` of random bits.
    ///
    /// Two draws on a 64-bit target, one on a 32-bit target (wasm32 and
    /// friends): there `usize` is already filled by a single `u32`, and
    /// `hi << 32` would be a constant shift past the width — which rustc
    /// rejects outright, so this cannot be written width-agnostically.
    #[inline]
    pub fn next_usize(&mut self) -> usize {
        let lo = self.next_u32() as usize;
        #[cfg(target_pointer_width = "64")]
        {
            let hi = self.next_u32() as usize;
            (hi << 32) | lo
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            lo
        }
    }

    /// Uniform-ish in `[0, n)` (n > 0).
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n <= 1 { 0 } else { self.next_usize() % n }
    }
}

impl Default for Random {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(windows, not(miri)))]
fn os_entropy(key: &mut [u32; 8]) -> bool {
    // RtlGenRandom via SystemFunction036 is the classic mimalloc path; the
    // supported modern equivalent is BCryptGenRandom with the system RNG.
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    // SAFETY: writing 32 bytes into our own buffer with the system RNG.
    let status = unsafe {
        BCryptGenRandom(
            core::ptr::null_mut(),
            key.as_mut_ptr().cast::<u8>(),
            (key.len() * 4) as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    status == 0
}

#[cfg(all(unix, not(miri)))]
fn os_entropy(key: &mut [u32; 8]) -> bool {
    // /dev/urandom: universally available and needs no libc feature probing.
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open("/dev/urandom") else {
        return false;
    };
    // SAFETY: reinterpreting our own [u32; 8] as bytes for the read.
    let buf =
        unsafe { core::slice::from_raw_parts_mut(key.as_mut_ptr().cast::<u8>(), key.len() * 4) };
    f.read_exact(buf).is_ok()
}

#[cfg(miri)]
fn os_entropy(_key: &mut [u32; 8]) -> bool {
    false // deterministic fallback under the interpreter
}

/// Wasm exposes no host RNG without JS bindings, so the documented weaker
/// fallback in [`Random::reseed`] is the only path. On `wasm32-unknown-unknown`
/// that fallback is weaker still: [`crate::prim::clock_now`] is a counter and
/// the thread id is a constant, leaving the stack address and the global
/// counter as the only varying inputs. Free-list encoding under `secure`
/// therefore has MUCH less entropy on wasm than on a native target — treat it
/// as corruption detection, not as an exploit-mitigation claim.
#[cfg(all(target_arch = "wasm32", not(miri)))]
fn os_entropy(_key: &mut [u32; 8]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_is_deterministic_and_varied() {
        let mut a = Random::new();
        let mut b = Random::new();
        a.seed_from([1, 2, 3, 4, 5, 6, 7, 8], 42);
        b.seed_from([1, 2, 3, 4, 5, 6, 7, 8], 42);
        let xs: Vec<u32> = (0..64).map(|_| a.next_u32()).collect();
        let ys: Vec<u32> = (0..64).map(|_| b.next_u32()).collect();
        assert_eq!(xs, ys, "same seed must give the same stream");
        assert!(xs.windows(2).any(|w| w[0] != w[1]), "stream is constant");
        // Different stream id ⇒ different output.
        let mut c = Random::new();
        c.seed_from([1, 2, 3, 4, 5, 6, 7, 8], 43);
        let zs: Vec<u32> = (0..64).map(|_| c.next_u32()).collect();
        assert_ne!(xs, zs);
    }

    #[test]
    fn reseed_produces_distinct_streams() {
        let mut a = Random::new();
        let mut b = Random::new();
        a.reseed();
        b.reseed();
        let xs: Vec<u32> = (0..8).map(|_| a.next_u32()).collect();
        let ys: Vec<u32> = (0..8).map(|_| b.next_u32()).collect();
        assert_ne!(xs, ys, "two reseeds collided");
    }
}
