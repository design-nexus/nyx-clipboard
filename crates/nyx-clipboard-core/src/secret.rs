//! A short secret (a verification code) in memory that is locked out of swap, kept out
//! of `Debug` output and zeroed when dropped.

use zeroize::Zeroize;

pub const CAPACITY: usize = 64;

pub struct Code {
    buf: Box<[u8; CAPACITY]>,
    len: usize,
    locked: bool,
}

impl Code {
    /// `None` when `value` is longer than [`CAPACITY`] bytes.
    pub fn new(value: &str) -> Option<Code> {
        let bytes = value.as_bytes();
        if bytes.len() > CAPACITY {
            return None;
        }
        let mut buf = Box::new([0u8; CAPACITY]);
        // SAFETY: the region is the box's own allocation, valid for CAPACITY bytes.
        let locked = unsafe { libc::mlock(buf.as_ptr().cast(), CAPACITY) } == 0;
        buf[..bytes.len()].copy_from_slice(bytes);
        Some(Code { buf, len: bytes.len(), locked })
    }

    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    /// Whether the memory is locked (it can't be if RLIMIT_MEMLOCK is exhausted).
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Constant-time comparison.
    pub fn same_as(&self, other: &[u8]) -> bool {
        if other.len() != self.len {
            return false;
        }
        self.buf[..self.len].iter().zip(other).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }
}

impl Drop for Code {
    fn drop(&mut self) {
        self.buf.zeroize();
        if self.locked {
            // SAFETY: the same region that was locked in `new`.
            unsafe { libc::munlock(self.buf.as_ptr().cast(), CAPACITY) };
        }
    }
}

impl std::fmt::Debug for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Code(…)")
    }
}

/// Keep this process's memory out of core dumps and away from other processes'
/// debuggers (`PR_SET_DUMPABLE`).
pub fn harden_process() {
    // SAFETY: prctl with these arguments has no memory effects.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_and_hides() {
        let c = Code::new("483920").unwrap();
        assert_eq!(c.expose(), "483920");
        assert!(c.same_as(b"483920"));
        assert!(!c.same_as(b"483921"));
        assert_eq!(format!("{c:?}"), "Code(…)");
        assert!(Code::new(&"x".repeat(65)).is_none());
    }
}
