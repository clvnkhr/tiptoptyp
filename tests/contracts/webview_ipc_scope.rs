//! Exercise the admission policy used before the native IPC body is read.
#[path = "../../vendor/wry/src/ipc_scope.rs"]
mod ipc_scope;

#[test]
fn shared_configuration_never_confers_the_owners_ipc_authority() {
    for owner in 1..=16 {
        for sender in 0..=16 {
            for main_frame in [false, true] {
                let sender = (sender != 0).then_some(sender);
                assert_eq!(
                    ipc_scope::main_frame_sender_allowed(owner, sender, main_frame),
                    main_frame && sender == Some(owner),
                    "owner={owner}, sender={sender:?}, main_frame={main_frame}"
                );
            }
        }
    }
}
