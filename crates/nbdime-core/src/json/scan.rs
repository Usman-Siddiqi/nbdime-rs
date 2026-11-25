//! Finding the next byte of a string that the parser or the serializer must look at, eight
//! bytes at a time.

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// Whether some byte of `word` is below `n`, for `n <= 0x80`.
#[inline]
fn has_byte_below(word: u64, n: u8) -> bool {
    word.wrapping_sub(ONES * u64::from(n)) & !word & HIGH != 0
}

#[inline]
fn has_byte(word: u64, byte: u8) -> bool {
    has_byte_below(word ^ (ONES * u64::from(byte)), 1)
}

/// Whether a JSON string must stop or escape at `byte`: a quote, a backslash, a control
/// character, and with `non_ascii` also DEL and every byte of a non-ASCII character.
#[inline]
fn is_special(byte: u8, non_ascii: bool) -> bool {
    byte < 0x20 || byte == b'"' || byte == b'\\' || (non_ascii && byte > b'~')
}

/// The number of leading bytes of `bytes` that are not special in the sense of [`is_special`].
#[inline]
pub(super) fn plain_prefix_len(bytes: &[u8], non_ascii: bool) -> usize {
    let mut at = 0;
    while let Some(chunk) = bytes.get(at..at + 8) {
        let word = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
        let special = has_byte_below(word, 0x20)
            || has_byte(word, b'"')
            || has_byte(word, b'\\')
            || (non_ascii && (word & HIGH != 0 || has_byte(word, 0x7f)));
        if special {
            break;
        }
        at += 8;
    }
    at + bytes[at..]
        .iter()
        .position(|&b| is_special(b, non_ascii))
        .unwrap_or(bytes.len() - at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(bytes: &[u8], non_ascii: bool) -> usize {
        bytes
            .iter()
            .position(|&b| is_special(b, non_ascii))
            .unwrap_or(bytes.len())
    }

    #[test]
    fn agrees_with_a_byte_by_byte_scan() {
        for non_ascii in [false, true] {
            for len in 0..20 {
                for at in 0..len {
                    for special in 0..=255u8 {
                        let mut bytes = vec![b'a'; len];
                        bytes[at] = special;
                        assert_eq!(
                            plain_prefix_len(&bytes, non_ascii),
                            reference(&bytes, non_ascii),
                            "{bytes:?} {non_ascii}"
                        );
                    }
                }
                let plain = vec![b'~'; len];
                assert_eq!(plain_prefix_len(&plain, non_ascii), len);
            }
        }
    }

    #[test]
    fn neighbouring_bytes_do_not_hide_or_fake_a_match() {
        // Bytes just above each boundary, next to bytes just below, in every lane.
        let pairs = [
            (0x20, 0x1f),
            (b'"' + 1, b'"'),
            (b'\\' + 1, b'\\'),
            (b'~', 0x7f),
            (b'!', 0x80),
        ];
        for non_ascii in [false, true] {
            for (above, below) in pairs {
                for at in 0..16 {
                    let mut bytes = vec![above; 16];
                    bytes[at] = below;
                    assert_eq!(
                        plain_prefix_len(&bytes, non_ascii),
                        reference(&bytes, non_ascii)
                    );
                }
            }
        }
    }
}
