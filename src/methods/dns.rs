// those header were wrote by gemini

mod _s {
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

pub fn bound_check(payload_max: usize) -> bool {
    payload_max > DNS_QUERY_LEN
}

#[inline(always)]
#[must_use]
pub fn dns_pad(ptr: *mut u8, n: usize) -> usize {
    super::align_check(ptr.addr());
    unsafe {
        core::ptr::copy(ptr.cast_const(), ptr.add(DNS_QUERY_LEN), n);
        core::ptr::copy(DNS_QUERY.as_ptr(), ptr, DNS_QUERY_LEN);
    };
    n.saturating_add(DNS_QUERY_LEN)
}

#[inline(always)]
#[must_use]
pub fn dns_unpad(ptr: *mut u8, n: usize) -> usize {
    super::align_check(ptr.addr());
    unsafe {
        core::ptr::copy(ptr.add(DNS_QUERY_LEN), ptr, n);
    };
    n.saturating_sub(DNS_QUERY_LEN)
}
