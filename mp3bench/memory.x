MEMORY
{
    /* 2MB Flash size default for typical RP2350 boards */
    FLASH : ORIGIN = 0x10000000, LENGTH = 2048K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS
{
    .start_block : ALIGN(4)
    {
        KEEP(*(.start_block));
    } > FLASH
} INSERT AFTER .vector_table;

/* move .text to start /after/ the boot info */
_stext = ADDR(.start_block) + SIZEOF(.start_block);

/* mp3dec's code runs from RAM, as mp3.sy's hot functions do, so the
   comparison isn't of flash against RAM. main copies it in. */
SECTIONS
{
    .mp3dec_text : ALIGN(4)
    {
        __smp3dec_text = .;
        *libmp3dec*:(.text .text.*)
        . = ALIGN(4);
        __emp3dec_text = .;
    } > RAM AT > FLASH
    __simp3dec_text = LOADADDR(.mp3dec_text);
} INSERT BEFORE .data;
