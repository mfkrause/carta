//! SHA-1 digest, used to give an embedded resource a content-addressed name.

/// The SHA-1 digest of `data` as a 40-character lowercase hex string.
#[allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "the hex-nibble and tail-buffer indices are bounded by the 16-entry table and the \
              128-byte tail; the length cast isolates the intended low 64 bits"
)]
#[must_use]
pub fn hex(data: &[u8]) -> String {
    const HEX: [u8; 16] = *b"0123456789abcdef";
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let mut blocks = data.chunks_exact(64);
    for block in &mut blocks {
        compress(&mut h, block);
    }

    // The message's tail — the final partial block, the `0x80` terminator, zero fill, and the
    // 64-bit big-endian bit length — is assembled on the stack rather than by copying the whole
    // input. A remainder of 56..=63 bytes leaves no room for the length in one block, so the tail
    // spans two.
    let remainder = blocks.remainder();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let tail_len = if remainder.len() <= 55 { 64 } else { 128 };
    let mut tail = [0u8; 128];
    tail[..remainder.len()].copy_from_slice(remainder);
    tail[remainder.len()] = 0x80;
    tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
    for block in tail[..tail_len].chunks_exact(64) {
        compress(&mut h, block);
    }

    let mut out = String::with_capacity(40);
    for word in h {
        for byte in word.to_be_bytes() {
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

/// Folds one 64-byte message block into the running state `h`.
#[allow(
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    reason = "the schedule indices are bounded by the fixed 80-word schedule and 64-byte block; \
              the single-letter names are the digest's own working-variable notation"
)]
fn compress(h: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (index, word) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes(word.try_into().unwrap_or([0; 4]));
    }
    for index in 16..80 {
        w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (index, &word) in w.iter().enumerate() {
        let (f, k) = match index {
            0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn matches_known_vectors() {
        assert_eq!(hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
        // 448 bits — a 56-byte message, which spills the length into a second block.
        assert_eq!(
            hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        // 896 bits — a 112-byte message spanning several blocks.
        assert_eq!(
            hex(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmn\
                  hijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
            "a49b2446a02c645bf419f995b67091253a04a259"
        );
    }

    #[test]
    fn padding_boundaries_agree_with_the_reference() {
        // Lengths straddling every mod-64 boundary — in particular 55 (fits one block) and 56
        // (forces a second) — must agree with the copy-based reference the stack tail replaced.
        for len in 0..=200usize {
            let data: Vec<u8> = (0..len)
                .map(|index| u8::try_from(index % 256).unwrap_or(0))
                .collect();
            assert_eq!(hex(&data), hex_reference(&data), "length {len}");
        }
    }

    /// The pre-refactor SHA-1: copies the input, pads in place, then digests. Kept in the test to
    /// pin the copy-free implementation to it across every padding boundary.
    #[allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::many_single_char_names,
        reason = "test-only reference formulation mirroring the original in-place padding"
    )]
    fn hex_reference(data: &[u8]) -> String {
        const HEX: [u8; 16] = *b"0123456789abcdef";
        let mut h: [u32; 5] = [
            0x6745_2301,
            0xEFCD_AB89,
            0x98BA_DCFE,
            0x1032_5476,
            0xC3D2_E1F0,
        ];
        let bit_len = (data.len() as u64) * 8;
        let mut message = data.to_vec();
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_len.to_be_bytes());
        for block in message.chunks_exact(64) {
            let mut w = [0u32; 80];
            for (index, word) in block.chunks_exact(4).enumerate() {
                w[index] = u32::from_be_bytes(word.try_into().unwrap_or([0; 4]));
            }
            for index in 16..80 {
                w[index] =
                    (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
            }
            let [mut a, mut b, mut c, mut d, mut e] = h;
            for (index, &word) in w.iter().enumerate() {
                let (f, k) = match index {
                    0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                    20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                    _ => (b ^ c ^ d, 0xCA62_C1D6),
                };
                let temp = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(word);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = temp;
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
        }
        let mut out = String::with_capacity(40);
        for word in h {
            for byte in word.to_be_bytes() {
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0f)] as char);
            }
        }
        out
    }
}
