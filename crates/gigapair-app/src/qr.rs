//! The pairing code as a QR, drawn as a grid of squares. Always dark on
//! light, whatever the theme, so phones can scan it.

use gpui_kit::{Div, IntoElement, ParentElement, Styled, div, hsla, px};

pub fn qr(code: &str) -> Option<Div> {
    let qr = qrcode::QrCode::with_error_correction_level(code.to_uppercase(), qrcode::EcLevel::L)
        .ok()?;
    let width = qr.width();
    let colors = qr.to_colors();
    let cell = px(4.);
    let dark = hsla(0., 0., 0., 1.);
    let light = hsla(0., 0., 1., 1.);
    let mut grid = div().flex().flex_col().p(cell * 4.).bg(light);
    for row in colors.chunks(width) {
        let mut line = div().flex().flex_row();
        for c in row {
            let color = if *c == qrcode::Color::Dark {
                dark
            } else {
                light
            };
            line = line.child(div().size(cell).bg(color).into_any_element());
        }
        grid = grid.child(line);
    }
    Some(grid)
}
