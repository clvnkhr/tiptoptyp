use tiptoptyp::themes::builtin;

#[test]
fn public_catalogue_contract_is_available_without_app_state() {
    assert_eq!(builtin::all().len(), 32);
    assert_eq!(
        builtin::find("catppuccin-mocha").map(|theme| theme.name),
        Some("Catppuccin Mocha")
    );
    assert_eq!(
        builtin::find("dracula-alucard").map(|theme| theme.name),
        Some("Dracula Alucard")
    );
    assert_eq!(builtin::for_mode(false).count(), 15);
    assert_eq!(builtin::for_mode(true).count(), 17);
    assert_eq!(builtin::default_for_mode(false).id, "tiptop-light");
    assert_eq!(builtin::default_for_mode(true).id, "tiptop-dark");
}
