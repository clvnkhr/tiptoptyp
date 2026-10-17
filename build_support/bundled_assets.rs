//! Architecture-independent assets shared by runtime lookup and package staging.
#![allow(dead_code)] // The packager and application consume different registry fields.

#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub name: &'static str,
    pub source: &'static str,
}

// A single list also generates the runtime's enum and development includes.
macro_rules! shared_asset_registry {
    ($consumer:ident) => {
        $consumer! {
            (NotoSans, "noto-sans.ttf", "assets/fonts/notosans/NotoSans[wdth,wght].ttf"),
            (NotoEmoji, "noto-emoji.ttf", "assets/fonts/notoemoji/NotoEmoji[wght].ttf"),
            (NotoSymbols, "noto-symbols.ttf", "assets/fonts/notosanssymbols/NotoSansSymbols[wght].ttf"),
            (NotoMath, "noto-math.ttf", "assets/fonts/notosansmath/NotoSansMath-Regular.ttf"),
            (NotoSymbols2, "noto-symbols2.ttf", "assets/fonts/notosanssymbols2/NotoSansSymbols2-Regular.ttf"),
            (NotoHebrew, "noto-hebrew.ttf", "assets/fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf"),
            (HackRegular, "hack-regular.ttf", "assets/fonts/hack/Hack-Regular.ttf"),
            (HackBold, "hack-bold.ttf", "assets/fonts/hack/Hack-Bold.ttf"),
            (HackItalic, "hack-italic.ttf", "assets/fonts/hack/Hack-Italic.ttf"),
            (HackBoldItalic, "hack-bold-italic.ttf", "assets/fonts/hack/Hack-BoldItalic.ttf"),
            (FileIcons, "file-icons.ttf", "assets/fonts/nerd/FileIcons.ttf"),
            (Detypify, "detypify.onnx", "assets/handwriting/detypify.onnx"),
            (DetexifySamples, "detexify-samples.bin", "assets/handwriting/detexify-samples.bin"),
        }
    };
}
#[allow(unused_imports)]
pub(crate) use shared_asset_registry;

macro_rules! define_registry {
    ($(($id:ident, $name:literal, $source:literal)),+ $(,)?) => {
        pub const SHARED_ASSETS: &[Asset] = &[
            $(Asset { name: $name, source: $source }),+
        ];
    };
}
shared_asset_registry!(define_registry);
