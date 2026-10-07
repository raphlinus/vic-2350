MEMORY
{
    FLASH (rx) : ORIGIN = 0x10000000, LENGTH = 1M
    RAM : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS
{
    .text : {
        KEEP(*(.vectors))
        KEEP(*(.image_def))
        *(.text*)
        *(.rodata*)
    } > FLASH

    .data : {
        . = ALIGN(4);
        _data_start = .;
        *(.ram*)
        *(.data*)
        . = ALIGN(4);
        _data_end = .;
    } > RAM AT > FLASH
    _data_source = LOADADDR(.data);

    .bss (NOLOAD) : {
        . = ALIGN(4);
        _bss_start = .;
        *(.bss*)
        . = ALIGN(4);
        _bss_end = .;
    } > RAM
}
_data_size_words = (_data_end - _data_start) / 4;
ASSERT(_data_size_words <= 0xffff, "_data_size_words >= 64k, can fix this by using ldr instead of movw")
ASSERT(_data_size_words > 0, "remove data copy section if zero, or add cbz guard")

_bss_size_words = (_bss_end - _bss_start) / 4;
ASSERT(_bss_size_words <= 0xffff, "_bss_size_words >= 64k, can fix this by using ldr instead of movw")
ASSERT(_bss_size_words > 0, "remove bss zero section if zero, or add cbz guard")
