//! Procedurally drawn microphone icon (no image assets required).

/// Signed distance to a rounded rectangle centred at the origin.
fn sd_rounded_rect(x: f32, y: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
    let qx = x.abs() - half_w + r;
    let qy = y.abs() - half_h + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[inline]
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Render an RGBA (straight alpha) microphone glyph on a round badge.
pub fn rgba(size: u32, active: bool) -> Vec<u8> {
    let (bg, fg): ([f32; 3], [f32; 3]) = if active {
        ([46.0, 230.0, 166.0], [12.0, 18.0, 24.0])
    } else {
        ([78.0, 84.0, 98.0], [230.0, 233.0, 238.0])
    };
    let s = size as f32;
    let c = s / 2.0;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let d_bg = (fx - c).hypot(fy - c) - (c - 0.5);
            let a_bg = coverage(d_bg);
            if a_bg <= 0.0 {
                continue;
            }
            let d_cap = sd_rounded_rect(fx - c, fy - (c - 0.09 * s), 0.13 * s, 0.25 * s, 0.13 * s);
            let d_arc = {
                let dx = fx - c;
                let dy = fy - (c + 0.03 * s);
                let ring = (dx.hypot(dy) - 0.26 * s).abs() - 0.035 * s;
                if dy < 0.0 { ring.max(-dy) } else { ring }
            };
            let d_stem = sd_rounded_rect(fx - c, fy - (c + 0.36 * s), 0.035 * s, 0.07 * s, 0.03 * s);
            let d_base = sd_rounded_rect(fx - c, fy - (c + 0.43 * s), 0.14 * s, 0.035 * s, 0.03 * s);
            let d_fg = d_cap.min(d_arc).min(d_stem).min(d_base);
            let a_fg = coverage(d_fg);
            let i = ((y * size + x) * 4) as usize;
            for k in 0..3 {
                let v = bg[k] + (fg[k] - bg[k]) * a_fg;
                px[i + k] = v.round().clamp(0.0, 255.0) as u8;
            }
            px[i + 3] = (a_bg * 255.0).round() as u8;
        }
    }
    px
}

pub fn egui_icon() -> egui::IconData {
    egui::IconData {
        rgba: rgba(64, true),
        width: 64,
        height: 64,
    }
}

pub fn tray_icon(active: bool) -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(rgba(32, active), 32, 32).expect("valid icon buffer")
}

/// Encode a multi-resolution Windows .ico (uncompressed 32-bit DIB entries).
pub fn ico_bytes() -> Vec<u8> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for &size in &sizes {
        let rgba = rgba(size, true);
        let mut dib = Vec::with_capacity(40 + (size * size * 4) as usize);
        // BITMAPINFOHEADER
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(size as i32).to_le_bytes());
        dib.extend_from_slice(&((size * 2) as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&(size * size * 4).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]);
        // pixel rows bottom-up, BGRA
        for y in (0..size).rev() {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // AND mask: 1 bpp, rows padded to 32 bits
        let row_bytes = (size as usize).div_ceil(32) * 4;
        dib.extend(std::iter::repeat_n(0u8, row_bytes * size as usize));
        images.push((size, dib));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, dib) in &images {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += dib.len() as u32;
    }
    for (_, dib) in images {
        out.extend_from_slice(&dib);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_opaque_centre_and_transparent_corners() {
        let px = rgba(32, true);
        assert_eq!(px[3], 0, "corner should be transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[centre + 3], 255);
    }

    #[test]
    fn ico_header_is_well_formed() {
        let ico = ico_bytes();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(ico[4], 7);
    }
}
