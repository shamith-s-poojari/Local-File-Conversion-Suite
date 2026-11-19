//! Best-effort hygiene for the password while this process holds it.
//!
//! No `zeroize` (a new dependency needs a yes), so this overwrites the bytes
//! by hand. It is best effort: `String` growth may have left an earlier copy
//! in freed memory, and the copy handed to the core is the core's to clear
//! (it keeps it in a `Zeroizing`). What it does is end the life of the copies
//! made here, so a core dump or a swapped page later holds fewer of them.

use std::sync::atomic::{compiler_fence, Ordering};

/// Overwrites `bytes` with zeros in a way the optimizer may not drop.
pub fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // Volatile: a plain store to memory that is freed next would be
        // removed as dead.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Overwrites a string's contents, then empties it.
pub fn wipe_string(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8, so the string stays valid.
    wipe(unsafe { s.as_bytes_mut() });
    s.clear();
}

/// A password that wipes itself when dropped.
#[derive(Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self {
        Secret(s)
    }

    /// For the one place that must hand a copy on (the core clears its own).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe_string(&mut self.0);
    }
}

// Never printed, even by a stray `{:?}`.
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_zeroes_every_byte() {
        let mut b = *b"hunter2";
        wipe(&mut b);
        assert_eq!(b, [0u8; 7]);
    }

    #[test]
    fn wipe_string_overwrites_before_clearing() {
        let mut s = String::from("hunter2");
        let ptr = s.as_ptr();
        let cap = s.capacity();
        wipe_string(&mut s);
        assert!(s.is_empty());
        // The buffer is still ours (clear keeps capacity): its bytes are zero.
        let after = unsafe { std::slice::from_raw_parts(ptr, cap.min(7)) };
        assert_eq!(after, [0u8; 7]);
    }

    #[test]
    fn debug_hides_the_value() {
        assert_eq!(format!("{:?}", Secret::new("hunter2".into())), "Secret(..)");
    }
}
