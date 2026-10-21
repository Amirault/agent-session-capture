//! Strip ANSI escape sequences from a styled Warp block buffer.

/// Warp stores block command/output text "ANSI-exploded" (e.g. per-character
/// bold: `\x1b[1mH\x1b[0m\x1b[1mi\x1b[0m`). Recover the clean text by walking
/// bytes and dropping escape sequences, then decode as UTF-8 (invalid
/// sequences replaced).
pub fn strip_ansi(input: Option<&[u8]>) -> String {
    let Some(buf) = input else { return String::new() };
    const CSI: u8 = b'[';
    const OSC: u8 = b']';
    const BEL: u8 = 0x07;
    const ESC: u8 = 0x1b;
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == ESC {
            i += 1;
            match buf.get(i) {
                Some(&CSI) => {
                    // CSI: ESC [ params... final byte in 0x40..0x7e
                    i += 1;
                    while i < buf.len() {
                        let c = buf[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&c) {
                            break;
                        }
                    }
                }
                Some(&OSC) => {
                    // OSC: ESC ] ... terminated by BEL or ST (ESC \)
                    i += 1;
                    while i < buf.len() {
                        let c = buf[i];
                        if c == BEL {
                            i += 1;
                            break;
                        }
                        if c == ESC && buf.get(i + 1) == Some(&0x5c) {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Two-character escape (ESC x) — skip the following byte if present.
                    if i < buf.len() {
                        i += 1;
                    }
                }
            }
        } else if b == BEL {
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
