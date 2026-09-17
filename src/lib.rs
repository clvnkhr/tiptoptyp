//! Reusable, UI-independent parts of tiptoptyp.

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod builtin_themes;
mod sublime_theme;
mod theme_transform;

/// Checked, reversible source translation for the optional MiTeX editing mode.
/// Ordinary Typst editing bypasses projection work.
pub mod mitex_projection;

/// Projection-aware document lifecycle and canonical save/service snapshots.
/// The application routes enabled-mode editing and service adapters through this API.
pub mod mitex_document;

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
