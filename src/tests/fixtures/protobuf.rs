//! Minimal protobuf encoder for test fixtures (builds known BLOBs to walk).

#![allow(dead_code)]

pub fn encode_varint(n: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut v = n;
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v > 0 {
            b |= 0x80;
        }
        bytes.push(b);
        if v == 0 {
            return bytes;
        }
    }
}

pub fn encode_tag(field_number: u32, wire_type: u32) -> Vec<u8> {
    encode_varint((field_number << 3) | wire_type)
}

pub fn encode_length_delimited(field_number: u32, data: &[u8]) -> Vec<u8> {
    let mut out = encode_tag(field_number, 2);
    out.extend(encode_varint(data.len() as u32));
    out.extend_from_slice(data);
    out
}

pub fn encode_string(field_number: u32, s: &str) -> Vec<u8> {
    encode_length_delimited(field_number, s.as_bytes())
}

pub fn encode_message(field_number: u32, sub: &[u8]) -> Vec<u8> {
    encode_length_delimited(field_number, sub)
}

pub fn encode_bytes(field_number: u32, data: &[u8]) -> Vec<u8> {
    encode_length_delimited(field_number, data)
}
