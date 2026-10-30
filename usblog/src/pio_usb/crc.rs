//! USB's CRCs, from usb_crc.sy.

extern "C" {
    #[link_name = "calc_usb_crc5"]
    fn sy_calc_usb_crc5(data: u32) -> u32;
    #[link_name = "update_crc_16"]
    fn sy_update_crc_16(crc: u32, data: u32) -> u32;
}

#[inline(always)]
pub fn calc_usb_crc5(data: u16) -> u8 {
    unsafe { sy_calc_usb_crc5(data as u32) as u8 }
}

#[inline(always)]
pub fn update_crc_16(crc: u16, data: u8) -> u16 {
    unsafe { sy_update_crc_16(crc as u32, data as u32) as u16 }
}
