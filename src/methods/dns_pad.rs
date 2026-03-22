use anyhow::{Result, bail};

use super::{MethodApply, MethodUndo};

mod _s {
    // those header were wrote by gemini
    pub const HEADER_LEN: usize = 12;
    // 1 byte (\x0C) + 12 bytes ("root-servers") +
    // 1 byte (\x03) + 3 bytes ("net") +
    // 1 byte (\x00) = 18 bytes
    const NAME_LEN: usize = 18;
    pub const Q_LEN: usize = NAME_LEN + 2 + 2; // Name + Type(2) + Class(2) = 22
}

pub const DNS_QUERY_LEN: usize = _s::HEADER_LEN + _s::Q_LEN * 2;
pub const DNS_QUERY: [u8; DNS_QUERY_LEN] = {
    const LABEL_1: &[u8] = b"root-servers";
    const LABEL_2: &[u8] = b"net";

    let mut buf = [0u8; DNS_QUERY_LEN];
    let mut idx = 0;

    // === 1. Header ===
    // ID = 0x0721
    const ID: [u8; 2] = [0x07, 0x21];
    let mut i = 0;
    while i < ID.len() {
        buf[idx] = ID[i];
        i += 1;
        idx += 1;
    }

    // Flags = 0x0100 (Standard Query, Recursion Desired)
    const FLAGS: [u8; 2] = [0x01, 0x10];
    let mut i = 0;
    while i < FLAGS.len() {
        buf[idx] = FLAGS[i];
        i += 1;
        idx += 1;
    }

    // QDCOUNT = 2
    const QDCOUNT: [u8; 2] = [0x00, 0x02];
    let mut i = 0;
    while i < QDCOUNT.len() {
        buf[idx] = QDCOUNT[i];
        i += 1;
        idx += 1;
    }

    // ANCOUNT, NSCOUNT, ARCOUNT
    idx += 6;

    // === 2. Questions Loop ===
    let mut i = 0;
    while i < 2 {
        // --- Name: root-servers.net ---

        // Label 1: "root-servers"
        buf[idx] = LABEL_1.len() as u8;
        idx += 1;

        let mut j = 0;
        while j < LABEL_1.len() {
            buf[idx] = LABEL_1[j];
            idx += 1;
            j += 1;
        }

        // Label 2: "net"
        buf[idx] = LABEL_2.len() as u8;
        idx += 1;

        let mut k = 0;
        while k < LABEL_2.len() {
            buf[idx] = LABEL_2[k];
            idx += 1;
            k += 1;
        }

        // End of Name (Null Terminator)
        buf[idx] = 0x00;
        idx += 1;

        // --- Type: NS (2) ---
        buf[idx] = 0x00;
        idx += 1;
        buf[idx] = 0x02;
        idx += 1;

        // --- Class: IN (1) ---
        buf[idx] = 0x00;
        idx += 1;
        buf[idx] = 0x01;
        idx += 1;

        i += 1;
    }

    buf
};

#[must_use]
pub fn payload_bound_check(payload_max: usize) -> bool {
    payload_max > DNS_QUERY_LEN
}

impl DnsPad {
    pub(super) const fn check_apply(buf_len: usize, n: usize) -> bool {
        buf_len > n + DNS_QUERY_LEN
    }

    pub(super) const fn check_undo(n: usize) -> bool {
        n >= DNS_QUERY_LEN
    }
}

// `local` -> self.apply() -> peer_padded
// peer_padded -> self.undo() -> `remote`
// vice versa
pub struct DnsPad;

impl MethodApply for DnsPad {
    #[inline(always)]
    fn apply(buf: &mut [u8], n: &mut usize) -> Result<()> {
        let len = *n;

        if !Self::check_apply(buf.len(), len) {
            bail!("dns pad overflow: cap={}, n={n}", buf.len());
        };

        let ptr = buf.as_mut_ptr();
        unsafe {
            core::ptr::copy(ptr.cast_const(), ptr.add(DNS_QUERY_LEN), len);
            core::ptr::copy_nonoverlapping(DNS_QUERY.as_ptr(), ptr, DNS_QUERY_LEN);
        };
        *n = len + DNS_QUERY_LEN;

        Ok(())
    }
}

impl MethodUndo for DnsPad {
    #[inline(always)]
    fn undo(buf: &mut [u8], n: &mut usize) -> Result<()> {
        if !Self::check_undo(*n) {
            bail!("dns unpad underflow: {n} < {DNS_QUERY_LEN}");
        };

        let len = *n - DNS_QUERY_LEN;

        let ptr = buf.as_mut_ptr();
        unsafe {
            core::ptr::copy(ptr.add(DNS_QUERY_LEN), ptr, len);
        };
        *n = len;

        Ok(())
    }
}

#[test]
fn test_dns_pad_roundtrip() {
    let mut buf = [0u8; 256];

    let original = b"hello dns pad";
    let mut n = original.len();

    buf[..n].copy_from_slice(original);

    DnsPad::apply(&mut buf, &mut n).unwrap();

    assert_eq!(n, original.len() + DNS_QUERY_LEN);

    assert_eq!(&buf[..DNS_QUERY_LEN], &DNS_QUERY);

    DnsPad::undo(&mut buf, &mut n).unwrap();

    assert_eq!(n, original.len());

    assert_eq!(&buf[..n], original);
}
