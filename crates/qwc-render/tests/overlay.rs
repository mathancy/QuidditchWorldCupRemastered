//! The front end's 2D pass: quads in the 512 × 512 screen, the colour's 0x80 as one, and the target's alpha as a
//! mask (type 4 writes only alpha, type 6 blends through it). Skips when the machine has no graphics adapter.

use qwc_render::overlay::{Overlay, OverlayQuad, OverlayTarget, OverlayVertex, BLENDS, OVERLAY_FORMAT};
use qwc_render::{wgpu, Gpu};

/// A quad over `x0..x1` × `y0..y1` (front-end pixels) with one colour.
fn quad(x0: f32, y0: f32, x1: f32, y1: f32, colour: [f32; 4], blend: u8, layer: u32) -> OverlayQuad {
    let v = |x, y, u, w| OverlayVertex { pos: [x, y], uv: [u, w], colour };
    OverlayQuad {
        texture: 1,
        blend,
        layer,
        corners: [v(x0, y1, 0.0, 1.0), v(x1, y1, 1.0, 1.0), v(x0, y0, 0.0, 0.0), v(x1, y0, 1.0, 0.0)],
    }
}

#[test]
fn quads_blend_by_type_and_layer() {
    let gpu = match Gpu::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut overlay = Overlay::new(&gpu.device, OVERLAY_FORMAT);
    overlay.upload(&gpu, 1, 1, 1, &[255, 255, 255, 255]);
    let target = OverlayTarget::new(&gpu.device, (64, 64));
    let quads = [
        // drawn last though listed first: layer 2. Blends through the mask: only where the mask allowed
        quad(0.0, 0.0, 512.0, 512.0, [0.5, 0.0, 0.0, 0.5], 6, 2),
        // layer 1: the mask, alpha only, over the left half
        quad(0.0, 0.0, 256.0, 512.0, [0.0, 0.0, 0.0, 1.0], 4, 1),
        // layer 0: grey, 0.25 doubled to 0.5
        quad(0.0, 0.0, 512.0, 512.0, [0.25, 0.25, 0.25, 0.0], 1, 0),
    ];
    overlay.draw(&gpu, &target.view, Some(wgpu::Color::TRANSPARENT), &quads);
    let px = target.read_rgba(&gpu).unwrap();
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    // left: red through the mask (dest alpha 1), alpha kept by type 6
    let left = at(10, 32);
    assert!(left[0] > 250 && left[1] < 5 && left[3] > 250, "{left:?}");
    // right: no mask (alpha 0), the grey stays
    let right = at(54, 32);
    assert!((right[0] as i32 - 128).abs() < 3 && (right[1] as i32 - 128).abs() < 3, "{right:?}");
    assert_eq!(BLENDS[4].writes, 0x8);
}
