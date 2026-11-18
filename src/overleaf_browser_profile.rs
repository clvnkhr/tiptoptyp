//! Browser-owned credentials isolated by the working copy's opaque profile ID.
//! IDs select storage; they contain neither credentials nor project addresses.
#[cfg(any(target_os = "windows", test))]
use std::path::{Path, PathBuf};

fn validate_profile_id(profile_id: Option<[u8; 16]>) -> Result<(), String> {
    if profile_id.is_some_and(|id| id == [0; 16]) {
        // WebKit's identified-store API throws for a nil UUID. Treat malformed
        // stored metadata as an ordinary setup error before calling native APIs.
        return Err("The saved Overleaf browser profile is invalid.".into());
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn directory(
    local_data: Option<&Path>,
    profile_id: Option<[u8; 16]>,
) -> Result<PathBuf, String> {
    validate_profile_id(profile_id)?;
    local_data.filter(|base| base.is_absolute())
        .map(|base| {
            let root = base.join("dev.tiptoptyp.editor").join("overleaf-browser");
            profile_id.map_or(root.clone(), |id| root.join(hex::encode(id)))
        })
        .ok_or_else(|| "The local application-data directory is unavailable. Overleaf cannot safely remember this session.".into())
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MacStore {
    General,
    Persistent([u8; 16]),
    Session([u8; 16]),
}

#[cfg(any(target_os = "macos", test))]
fn mac_store(profile_id: Option<[u8; 16]>, persistent_profiles_available: bool) -> MacStore {
    match (profile_id, persistent_profiles_available) {
        (None, _) => MacStore::General,
        (Some(id), true) => MacStore::Persistent(id),
        (Some(id), false) => MacStore::Session(id),
    }
}

#[cfg(any(target_os = "macos", test))]
struct ProfileStores<T> {
    stores: std::collections::BTreeMap<[u8; 16], T>,
}

#[cfg(any(target_os = "macos", test))]
impl<T: Clone> ProfileStores<T> {
    fn retained(&mut self, profile: [u8; 16], create: impl FnOnce() -> T) -> T {
        self.stores.entry(profile).or_insert_with(create).clone()
    }
}

#[cfg(target_os = "macos")]
pub(super) fn configure_macos(
    builder: wry::WebViewBuilder<'_>,
    profile_id: Option<[u8; 16]>,
) -> Result<wry::WebViewBuilder<'_>, String> {
    use objc2::{ClassType as _, MainThreadMarker, msg_send, sel};
    use objc2_web_kit::{WKWebViewConfiguration, WKWebsiteDataStore};
    use wry::WebViewBuilderExtMacos as _;

    validate_profile_id(profile_id)?;
    let main = MainThreadMarker::new().ok_or("Browser profiles require the main thread.")?;
    // Wry ignores identifiers before macOS 14 and would choose the shared store.
    // Test availability ourselves, so older systems never mix workspace logins.
    // SAFETY: This queries a public class method on WebKit's main thread; the
    // method is invoked by Wry only when this check reports it available.
    let persistent_profiles_available: bool = unsafe {
        msg_send![WKWebsiteDataStore::class(), respondsToSelector: sel!(dataStoreForIdentifier:)]
    };
    Ok(match mac_store(profile_id, persistent_profiles_available) {
        MacStore::General => builder,
        store @ (MacStore::Persistent(_) | MacStore::Session(_)) => {
            // Dropping the last named store alongside its WebView can lose a
            // just-issued login while WebKit's persistence is still pending.
            // Retain the store across offline periods/workspace switches too.
            // Each view still receives its own configuration/content controller,
            // so old initialization scripts and IPC owners are never shared.
            let store = scoped_store(store, main);
            // SAFETY: Both configuration and store are created, configured and
            // consumed on the AppKit main thread and remain retained by WebKit.
            let configuration = unsafe { WKWebViewConfiguration::new(main) };
            unsafe { configuration.setWebsiteDataStore(&store) };
            builder.with_webview_configuration(configuration)
        }
    })
}

#[cfg(target_os = "macos")]
fn scoped_store(
    store: MacStore,
    main: objc2::MainThreadMarker,
) -> objc2::rc::Retained<objc2_web_kit::WKWebsiteDataStore> {
    use objc2_foundation::NSUUID;
    use objc2_web_kit::WKWebsiteDataStore;
    let (profile, persistent) = match store {
        MacStore::General => unreachable!("The general profile uses WebKit's default store"),
        MacStore::Persistent(id) => (id, true),
        MacStore::Session(id) => (id, false),
    };
    thread_local! {
        // Accessed only during explicit view creation, never on editor frames.
        // Retain only each visited profile's data store for the app lifetime,
        // including persistent stores; no views, configurations or credentials.
        static STORES: std::cell::RefCell<ProfileStores<
            objc2::rc::Retained<WKWebsiteDataStore>,
        >> = const { std::cell::RefCell::new(ProfileStores {
            stores: std::collections::BTreeMap::new(),
        }) };
    }
    STORES.with(|stores| {
        stores.borrow_mut().retained(profile, || {
            // SAFETY: Creation and retention happen on WebKit's main thread.
            // The identified-store class method is used only after its public
            // availability check; macOS 12/13 gets an isolated in-memory store.
            unsafe {
                if persistent {
                    WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(profile), main)
                } else {
                    WKWebsiteDataStore::nonPersistentDataStore(main)
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_storage_uses_a_fixed_user_directory_without_executable_or_project_paths() {
        let base = std::env::temp_dir();
        let expected = base.join("dev.tiptoptyp.editor/overleaf-browser");
        assert_eq!(directory(Some(&base), None).unwrap(), expected);
        assert!(directory(None, None).is_err());
        assert!(directory(Some(Path::new("")), None).is_err());
        assert!(directory(Some(Path::new("relative/profile")), None).is_err());
    }

    #[test]
    fn workspace_profiles_are_stable_distinct_opaque_children_of_the_user_directory() {
        let base = std::env::temp_dir();
        let first_id = [0x12; 16];
        let second_id = [0x34; 16];
        let first = directory(Some(&base), Some(first_id)).unwrap();
        let second = directory(Some(&base), Some(second_id)).unwrap();
        let general = directory(Some(&base), None).unwrap();
        assert_eq!(first, directory(Some(&base), Some(first_id)).unwrap());
        assert_ne!(first, second);
        assert_ne!(first, general);
        assert_eq!(first.parent(), Some(general.as_path()));
        assert_eq!(
            first.file_name().unwrap(),
            "12121212121212121212121212121212"
        );
    }

    #[test]
    fn nil_profile_ids_fail_before_native_store_or_directory_selection() {
        assert!(validate_profile_id(Some([0; 16])).is_err());
        assert!(directory(Some(&std::env::temp_dir()), Some([0; 16])).is_err());
        assert!(validate_profile_id(None).is_ok());
        assert!(validate_profile_id(Some([1; 16])).is_ok());
        let mut almost_zero = [0; 16];
        almost_zero[15] = 1;
        assert!(validate_profile_id(Some(almost_zero)).is_ok());
    }

    #[test]
    fn older_macos_never_substitutes_shared_cookies_for_an_isolated_profile() {
        let first = [0x12; 16];
        let second = [0x34; 16];
        assert_eq!(mac_store(Some(first), false), MacStore::Session(first));
        assert_eq!(mac_store(Some(second), false), MacStore::Session(second));
        assert_eq!(mac_store(Some(first), true), MacStore::Persistent(first));
        assert_eq!(mac_store(None, false), MacStore::General);
        assert_eq!(mac_store(None, true), MacStore::General);
    }

    #[test]
    fn scoped_store_survives_view_drop_and_reuses_identity_without_reinitializing() {
        use std::{cell::Cell, rc::Rc};
        let mut stores = ProfileStores {
            stores: std::collections::BTreeMap::new(),
        };
        let created = Cell::new(0);
        let first = stores.retained([1; 16], || {
            created.set(created.get() + 1);
            Rc::new(())
        });
        let identity = Rc::downgrade(&first);
        drop(first); // Work offline removes the last WebView, not its store.
        assert!(identity.upgrade().is_some());
        for _ in 0..1000 {
            let reopened =
                stores.retained([1; 16], || panic!("Recreated a remembered login store"));
            assert!(Rc::ptr_eq(&identity.upgrade().unwrap(), &reopened));
        }
        assert_eq!(created.get(), 1);
        assert_eq!(stores.stores.len(), 1);
        let other = stores.retained([2; 16], || Rc::new(()));
        assert!(!Rc::ptr_eq(&identity.upgrade().unwrap(), &other));
        assert_eq!(stores.stores.len(), 2);
        drop(stores);
        assert!(identity.upgrade().is_none());
    }
}
