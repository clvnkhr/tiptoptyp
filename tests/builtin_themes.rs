#[path = "../src/builtin_themes.rs"]
mod builtin_themes;
#[allow(dead_code)]
#[path = "../src/sublime_theme.rs"]
mod sublime_theme;

#[test]
fn public_catalogue_contract_is_available_without_app_state() {
    assert_eq!(builtin_themes::all().len(), 32);
    assert_eq!(
        builtin_themes::find("catppuccin-mocha").map(|theme| theme.name),
        Some("Catppuccin Mocha")
    );
    assert_eq!(
        builtin_themes::find("dracula-alucard").map(|theme| theme.name),
        Some("Dracula Alucard")
    );
    assert_eq!(builtin_themes::for_mode(false).count(), 15);
    assert_eq!(builtin_themes::for_mode(true).count(), 17);
    assert_eq!(builtin_themes::default_for_mode(false).id, "tiptop-light");
    assert_eq!(builtin_themes::default_for_mode(true).id, "tiptop-dark");
}
