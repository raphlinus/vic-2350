// Copied from ~/garden/nibnib (src/decode.rs at 840c9df).

#[derive(Debug)]
pub enum Error {
    /// Not even enough bytes for a header
    Truncated,
    /// Width must be positive and even
    InvalidWidth,
    /// Size doesn't match width and height in header
    SizeMismatch,
}

fn read_u16(bytes: &[u8]) -> u16 {
    if bytes.len() < 2 {
        0
    } else {
        if let Ok(raw) = bytes[..2].try_into() {
            u16::from_le_bytes(raw)
        } else {
            0
        }
    }
}

fn read_u32(bytes: &[u8]) -> u32 {
    if bytes.len() < 2 {
        0
    } else {
        if let Ok(raw) = bytes[..4].try_into() {
            u32::from_le_bytes(raw)
        } else {
            0
        }
    }
}

/// Decode to 24 bit RGB
pub fn decode(src: &[u8]) -> Result<Vec<u8>, Error> {
    if src.len() < 1028 {
        return Err(Error::Truncated);
    }
    let width = read_u16(&src[0..2]);
    if width == 0 || width % 2 == 1 {
        return Err(Error::InvalidWidth);
    }
    let height = read_u16(&src[2..4]);
    let stride = crate::encode::line_stride(width as u32);
    if src.len() != stride * height as usize + 1028 {
        return Err(Error::SizeMismatch);
    }
    let mut result = Vec::with_capacity(width as usize * height as usize * 3);
    let pal: [u32; 256] = core::array::from_fn(|i| read_u32(&src[4 + 4 * i..]));
    for y in 0..height as usize {
        let mut i = 1028 + y * stride;
        let mut pix = read_u32(&src[i..]);
        two_565_pixels(&mut result, pix);
        i += 4;
        for _x in 1..(width / 2) {
            let inp = src[i];
            i += 1;
            pix = ((pix >> 16) * 0x10001).wrapping_add(pal[inp as usize]);
            two_565_pixels(&mut result, pix);
        }
    }
    Ok(result)
}

fn two_565_pixels(out: &mut Vec<u8>, pix: u32) {
    fn one_pix(out: &mut Vec<u8>, pix: u32) {
        let r0 = pix & 0xf800;
        out.push(((r0 >> 8) | (r0 >> 13)) as u8);
        let g0 = pix & 0x7e0;
        out.push(((g0 >> 3) | (g0 >> 9)) as u8);
        let b0 = pix & 0x1f;
        out.push(((b0 << 3) | (b0 >> 2)) as u8);
    }
    one_pix(out, pix);
    one_pix(out, pix >> 16);
}
