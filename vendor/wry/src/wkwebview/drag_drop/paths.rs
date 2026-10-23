// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

// Shared verbatim with the app's macOS file-drop integration tests.
use std::path::PathBuf;

use objc2::ClassType;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::{NSArray, NSURL};

pub(super) fn collect_paths(pasteboard: &NSPasteboard) -> Vec<PathBuf> {
    let classes = NSArray::arrayWithObject(NSURL::class());
    // SAFETY: NSURL implements NSPasteboardReading, and no reading options are passed.
    let Some(urls) = (unsafe { pasteboard.readObjectsForClasses_options(&classes, None) }) else {
        return Vec::new();
    };
    urls.into_iter()
        .filter_map(|object| {
            let url = object.downcast::<NSURL>().ok()?;
            if !url.isFileURL() {
                return None;
            }
            url.to_file_path()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboardTypeFileURL, NSPasteboardWriting};
    use objc2_foundation::NSString;

    #[test]
    fn file_drop_keeps_multiple_paths_and_literal_url_characters() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let expected = ["/tmp/a folder/数学.typ", "/tmp/hash#percent%.tex"];
        let urls = expected.map(|path| NSURL::fileURLWithPath(&NSString::from_str(path)));
        let items: Vec<_> = urls
            .iter()
            .map(|url| ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&**url))
            .collect();
        assert!(pasteboard.writeObjects(&NSArray::from_slice(&items)));
        assert_eq!(collect_paths(&pasteboard), expected.map(PathBuf::from));
        pasteboard.clearContents();
    }

    #[test]
    fn file_drop_ignores_web_urls_and_plain_text() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let web = NSURL::URLWithString(&NSString::from_str("https://example.com/a.typ")).unwrap();
        let text = NSString::from_str("This is not a file drop");
        let items = NSArray::from_slice(&[
            ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*web),
            ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*text),
        ]);
        assert!(pasteboard.writeObjects(&items));
        assert!(collect_paths(&pasteboard).is_empty());
        pasteboard.clearContents();
    }

    #[test]
    fn file_drop_rejects_paths_that_cannot_be_represented_safely() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let path = format!("file:///tmp/{}", "a".repeat(4096));
        let url = NSURL::URLWithString(&NSString::from_str(&path)).unwrap();
        let items =
            NSArray::from_slice(&[ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*url)]);
        assert!(pasteboard.writeObjects(&items));
        assert!(collect_paths(&pasteboard).is_empty());
        pasteboard.clearContents();
    }

    #[test]
    fn file_drop_ignores_empty_or_malformed_pasteboards() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        assert!(collect_paths(&pasteboard).is_empty());
        assert!(
            pasteboard.setString_forType(&NSString::from_str("http://[invalid"), unsafe {
                NSPasteboardTypeFileURL
            },)
        );
        assert!(collect_paths(&pasteboard).is_empty());
        pasteboard.clearContents();
    }
}
