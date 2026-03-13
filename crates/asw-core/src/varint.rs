/// LEB128 unsigned varint encoding/decoding for compact graph adjacency.

/// Encode a u32 as LEB128 varint, appending bytes to `buf`.
pub fn encode(mut value: u32, buf: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decode a LEB128 varint from `data[pos..]`, returning (value, new_pos).
/// Panics on malformed input (truncated varint).
pub fn decode(data: &[u8], mut pos: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0;
    loop {
        let byte = data[pos];
        pos += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_zero() {
        let mut buf = Vec::new();
        encode(0, &mut buf);
        assert_eq!(buf.len(), 1);
        let (val, pos) = decode(&buf, 0);
        assert_eq!(val, 0);
        assert_eq!(pos, 1);
    }

    #[test]
    fn roundtrip_single_byte() {
        let mut buf = Vec::new();
        encode(127, &mut buf);
        assert_eq!(buf.len(), 1);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, 127);
    }

    #[test]
    fn roundtrip_two_bytes() {
        let mut buf = Vec::new();
        encode(128, &mut buf);
        assert_eq!(buf.len(), 2);
        let (val, pos) = decode(&buf, 0);
        assert_eq!(val, 128);
        assert_eq!(pos, 2);
    }

    #[test]
    fn roundtrip_large() {
        let mut buf = Vec::new();
        encode(40_000_000, &mut buf);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, 40_000_000);
    }

    #[test]
    fn roundtrip_max() {
        let mut buf = Vec::new();
        encode(u32::MAX, &mut buf);
        assert_eq!(buf.len(), 5);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, u32::MAX);
    }

    #[test]
    fn multiple_values_sequential() {
        let mut buf = Vec::new();
        encode(100, &mut buf);
        encode(200, &mut buf);
        encode(300, &mut buf);
        let (v1, p1) = decode(&buf, 0);
        let (v2, p2) = decode(&buf, p1);
        let (v3, _) = decode(&buf, p2);
        assert_eq!((v1, v2, v3), (100, 200, 300));
    }
}
