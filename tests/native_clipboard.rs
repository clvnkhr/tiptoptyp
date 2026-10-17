use eframe::{egui, window_host::clipboard_request_event};

#[test]
fn native_paste_reads_once_and_normalizes_multiline_tex() {
    let mut reads = 0;
    let event = clipboard_request_event(&egui::ViewportCommand::RequestPaste, || {
        reads += 1;
        Some("\\begin{tabular}{ll}\r\nA & B \\\\\r\n\\end{tabular}".to_owned())
    });
    assert_eq!(reads, 1);
    assert_eq!(
        event,
        Some(egui::Event::Paste(
            "\\begin{tabular}{ll}\nA & B \\\\\n\\end{tabular}".to_owned()
        ))
    );
}

#[test]
fn unavailable_or_empty_clipboard_does_not_deliver_input() {
    for contents in [None, Some(String::new())] {
        assert_eq!(
            clipboard_request_event(&egui::ViewportCommand::RequestPaste, || contents),
            None
        );
    }
}

#[test]
fn native_cut_and_copy_do_not_read_the_clipboard() {
    for (command, expected) in [
        (egui::ViewportCommand::RequestCut, egui::Event::Cut),
        (egui::ViewportCommand::RequestCopy, egui::Event::Copy),
    ] {
        assert_eq!(
            clipboard_request_event(&command, || panic!("unexpected clipboard read")),
            Some(expected)
        );
    }
}

#[test]
fn unrelated_window_commands_do_not_create_clipboard_input() {
    for command in [
        egui::ViewportCommand::Focus,
        egui::ViewportCommand::Screenshot(Default::default()),
        egui::ViewportCommand::Title("Table editor".to_owned()),
    ] {
        assert_eq!(
            clipboard_request_event(&command, || panic!("unexpected clipboard read")),
            None
        );
    }
}
