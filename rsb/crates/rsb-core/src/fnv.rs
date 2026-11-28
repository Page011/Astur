//! FNV-1a, 64 bit. Used to tell file versions apart: the same bytes give the
//! same number in every build and every process (the standard library's
//! hasher does not promise that). It detects edits, not tampering.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// The FNV-1a 64 bit hash of `bytes`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// A hash built up from several pieces, as if they were one byte string.
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a64(u64);

impl Default for Fnv1a64 {
    fn default() -> Self {
        Fnv1a64(OFFSET)
    }
}

impl Fnv1a64 {
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_test_vectors() {
        // From the FNV reference test suite.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"b"), 0xaf63_df4c_8601_f1a5);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn one_changed_byte_changes_the_hash_and_a_byte_order_mark_counts() {
        let plain = fnv1a64(b"height = 36\n");
        assert_ne!(plain, fnv1a64(b"height = 37\n"));
        assert_ne!(plain, fnv1a64(b"\xef\xbb\xbfheight = 36\n"));
        assert_eq!(plain, fnv1a64(b"height = 36\n"));
    }

    #[test]
    fn pieces_hash_like_the_whole() {
        let mut h = Fnv1a64::default();
        h.write(b"foo");
        h.write(b"bar");
        assert_eq!(h.finish(), fnv1a64(b"foobar"));
    }
}
