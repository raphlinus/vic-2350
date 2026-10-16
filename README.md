# vic-2350

This project is an envisioning of an alternate universe mid-80s home computer. It runs on RP2350 boards, with the Adafruit Fruit Jam as the primary target. The name is an homage to the [VIC-II chip] at the core of the Commodore 64 computer.

It is primarily written in [symbolasm], a modern syntax for ARM assembly language.

A roadmap is coming soon.

## Beam racing

In the early 80s, there were two camps regarding video output: a dumb framebuffer and more sophisticated on-the-fly video processing. Rendering tiles and sprites on the fly was ideally suited to 8 bit video games, enabling smoothly interactive graphics without much effort by the CPU.

Text modes were in some ways a special case of on the fly rendering. Invariably limited to fixed-width cells, video RAM contained character indices and additional attributes, and during a scanline the video hardware would look up the character index in a font RAM before scanning out. This mechanism actually has a lot in common with video game tile rendering, and C64 games often abused the text rendering capabilities to draw game content.

The heart of vic-2350 is a beam racing renderer, inspired by 80s hardware. The RP2350 has a rare confluence of factors that make beam racing appealing: framebuffers take too much RAM (so the resolution and color depth is limited), plus the chip has enough horsepower to render quite interesting graphics on the fly.

This project is a continuation of [pico-dvi-rs], with a much greater emphasis on bare metal programming.

## License

Licensed under either of

- Apache License, Version 2.0
  ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license
  ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

[symbolasm]: https://github.com/raphlinus/symbolasm
[VIC-II chip]: https://en.wikipedia.org/wiki/MOS_Technology_VIC-II
[pico-dvi-rs]: https://github.com/DusterTheFirst/pico-dvi-rs
