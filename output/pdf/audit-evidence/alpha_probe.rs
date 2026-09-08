use ecolor::Color32;
mod theme { use ecolor::Color32; pub struct Palette { pub editor_background: Color32 } pub fn syntax_palette(_:bool) -> Palette { Palette { editor_background: Color32::WHITE } } }
fn composite_over_editor(color: Color32, dark: bool) -> Color32 {
    if color.a() == 255 {
        return color;
    }
    let base = theme::syntax_palette(dark).editor_background;
    let alpha = color.a() as u16;
    let blend = |foreground: u8, background: u8| {
        ((foreground as u16 * alpha + background as u16 * (255 - alpha) + 127) / 255) as u8
    };
    Color32::from_rgb(
        blend(color.r(), base.r()),
        blend(color.g(), base.g()),
        blend(color.b(), base.b()),
    )
}

fn main() { let color = Color32::from_rgba_unmultiplied(255,0,0,128); println!("input premultiplied {:?}; displayed {:?}; expected sRGB [255,127,127]", color.to_array(), composite_over_editor(color,false).to_array()); }