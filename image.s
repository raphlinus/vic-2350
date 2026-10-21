@ Copyright 2026 Raph Levien
@ SPDX-License-Identifier: Apache-2.0 OR MIT

@ The compressed image for render_line_image, in RAM. It's included as
@ binary here, since symbolasm has no equivalent of include_bytes!.

.section .data.image_data, "aw"
.align 2
.global image_data
image_data:
.incbin "imgdata"
