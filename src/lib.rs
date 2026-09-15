//! Reusable, UI-independent parts of tiptoptyp.

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod builtin_themes;
mod sublime_theme;
mod theme_transform;

/// Theme catalogue, import, and transformation APIs.
pub mod themes {
    /// Bundled theme catalogue.
    pub mod builtin {
        pub use crate::builtin_themes::{BuiltinTheme, all, default_for_mode, find, for_mode};
    }

    /// Sublime Text and TextMate theme import.
    pub mod sublime {
        pub use crate::sublime_theme::{
            ImportError, ImportedTheme, Rgba, SemanticPalette, ThemeFormat, import_bytes,
            import_path,
        };
    }

    /// Deterministic transformations over complete themes.
    pub mod transform {
        pub use crate::theme_transform::{ThemeColorAdjustments, ThemeTransform};
    }
}
