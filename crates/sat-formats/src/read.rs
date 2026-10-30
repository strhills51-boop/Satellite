use std::io::Read;

pub(crate) fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}
pub(crate) fn read_exact_vec<R: Read>(r: &mut R, n: usize) -> std::io::Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(v)
}
/// Bytes up to the first NUL, lossily decoded (plugin strings are Windows-1252 in practice).
pub(crate) fn zstring(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let s = &b[..end];
    match std::str::from_utf8(s) {
        Ok(v) => v.to_string(),
        Err(_) => s.iter().map(|&c| c as char).collect(),
    }
}
