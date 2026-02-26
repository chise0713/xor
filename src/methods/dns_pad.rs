use anyhow::{Result, bail};

use super::{ApplyProof, MethodApply, MethodUndo, UndoProof};

mod _s {
    // those header were wrote by gemini
    pub const HEADER_LEN: usize = 12;
    // 1 byte (\x0C) + 12 bytes ("root-servers") +
    // 1 byte (\x03) + 3 bytes ("net") +
    // 1 byte (\x00) = 18 bytes
    pub const NAME_LEN: usize = 18;
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

// ZST proof token with private field,
// can only be constructed by the module
mod proof {
    use super::*;

    pub struct DnsPadApplyProof {
        _token: (),
    }

    impl ApplyProof for DnsPadApplyProof {
        type Method = DnsPad;
    }

    pub struct DnsPadUndoProof {
        _token: (),
    }

    impl UndoProof for DnsPadUndoProof {
        type Method = DnsPad;
    }

    impl DnsPad {
        pub fn check_apply(buf_len: usize, n: usize) -> Option<DnsPadApplyProof> {
            (buf_len > n + DNS_QUERY_LEN).then_some(DnsPadApplyProof { _token: () })
        }

        pub fn check_undo(n: usize) -> Option<DnsPadUndoProof> {
            (n >= DNS_QUERY_LEN).then_some(DnsPadUndoProof { _token: () })
        }
    }
}

// `local` -> self.apply() -> peer_padded
// peer_padded -> self.undo() -> `remote`
// vice versa
pub struct DnsPad;

impl MethodApply for DnsPad {
    #[inline(always)]
    unsafe fn apply_unsafe<P>(_proof: P, ptr: *mut u8, n: &mut usize)
    where
        P: ApplyProof<Method = Self>,
    {
        let len = *n;
        unsafe {
            core::ptr::copy(ptr.cast_const(), ptr.add(DNS_QUERY_LEN), len);
            core::ptr::copy(DNS_QUERY.as_ptr(), ptr, DNS_QUERY_LEN);
        };
        *n = len + DNS_QUERY_LEN;
    }

    #[inline(always)]
    fn apply(buf: &mut [u8], n: &mut usize) -> Result<()> {
        let Some(proof) = Self::check_apply(buf.len(), *n) else {
            bail!("dns pad overflow: cap={}, n={n}", buf.len());
        };
        unsafe { Self::apply_unsafe(proof, buf.as_mut_ptr(), n) };
        Ok(())
    }
}

impl MethodUndo for DnsPad {
    #[inline(always)]
    unsafe fn undo_unsafe<P>(_proof: P, ptr: *mut u8, n: &mut usize)
    where
        P: UndoProof<Method = Self>,
    {
        let len = *n;
        unsafe {
            core::ptr::copy(ptr.add(DNS_QUERY_LEN), ptr, len);
        };
        *n = len - DNS_QUERY_LEN
    }

    #[inline(always)]
    fn undo(buf: &mut [u8], n: &mut usize) -> Result<()> {
        let Some(proof) = Self::check_undo(*n) else {
            bail!("dns unpad underflow: {n} < {DNS_QUERY_LEN}");
        };
        unsafe { Self::undo_unsafe(proof, buf.as_mut_ptr(), n) };
        Ok(())
    }
}
