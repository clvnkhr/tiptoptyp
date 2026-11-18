use super::*;

#[test]
fn device_progress_exposes_only_the_expected_code_and_allowed_verification_page() {
    let output = b"noise secret-token\n! First copy your one-time code: ABCD-EFGH\nOpen this URL to continue in your web browser: https://github.com/login/device\n";
    assert_eq!(
        device_progress(output, false),
        Some(("ABCD-EFGH".into(), DEVICE_URL.into()))
    );
    assert!(
        device_progress(
            b"! First copy your one-time code: secret-token\nhttps://github.com/login/device",
            false
        )
        .is_none()
    );
    assert!(
        device_progress(
            b"! First copy your one-time code: ABCD-EFGH\nhttps://attacker.invalid/login/device",
            false
        )
        .is_none()
    );
    assert!(
        device_progress(
            b"! First copy your one-time code: ABCD-EFGH\nhttp://127.0.0.1:41234/login/device",
            false
        )
        .is_none()
    );
    assert!(
        device_progress(
            b"! First copy your one-time code: ABCD-EFGH\nhttp://127.0.0.1:41234/login/device",
            true
        )
        .is_some()
    );
    for address in [
        "https://github.com.evil.invalid/login/device",
        "https://github.com/login/device?token=secret",
        "http://localhost.evil.invalid/login/device",
        "http://user:pass@127.0.0.1/login/device",
    ] {
        assert!(allowed_verification_uri(address, false).is_none());
        assert!(allowed_verification_uri(address, true).is_none());
    }
}

#[test]
fn names_and_personal_accounts_are_validated_before_api_requests() {
    for name in ["paper-backup", ".github", "project_2", "Research.2026"] {
        assert!(validate_name(name).is_ok());
    }
    for name in ["", ".", "..", ".git", "../other", "a/b", "a b", "a\nrepo"] {
        assert!(validate_name(name).is_err());
    }
    for login in ["researcher", "user-123"] {
        assert!(valid_login(login));
    }
    for login in ["", "-user", "user-", "user/repo", "user name"] {
        assert!(!valid_login(login));
    }
}

#[test]
fn plaintext_fallback_is_removed_without_logging_out_or_losing_account_metadata() {
    let root = tempfile::tempdir().unwrap();
    let tool = Prepared::for_test(root.path().join("gh"), root.path().join("auth")).unwrap();
    let hosts = tool.config_dir.join("hosts.yml");
    fs::write(&hosts, "github.com:\n    user: token-user\n    oauth_token: ghp_fake_secret\n    git_protocol: https\n    users:\n        token-user:\n            oauth_token: ghp_fake_secret\n            other: retained\n        token:\n            other: also-retained\n").unwrap();
    let error = reject_plaintext_profile(&tool).unwrap_err();
    assert!(!error.contains("ghp_fake_secret"));
    let clean = fs::read_to_string(&hosts).unwrap();
    assert!(!clean.contains("oauth_token"));
    assert!(!clean.contains("ghp_fake_secret"));
    assert!(clean.contains("token-user"));
    assert!(clean.contains("        token:\n"));
    assert!(clean.contains("other: retained"));
    assert!(reject_plaintext_profile(&tool).is_ok());
}

#[test]
fn helper_arguments_keep_paths_literal_and_do_not_embed_credentials() {
    let root = tempfile::tempdir().unwrap();
    let tool = Prepared::for_test(
        root.path().join("a 'quoted' folder/gh"),
        root.path().join("auth"),
    )
    .unwrap();
    let helper = credential_helper(&tool).unwrap();
    assert!(helper.starts_with("!unset GH_TOKEN GITHUB_TOKEN"));
    assert!(helper.contains("GH_PROMPT_DISABLED=1"));
    assert!(helper.contains("'\"'\"'"));
    assert!(helper.ends_with("auth git-credential"));
}

#[test]
fn another_window_cannot_queue_an_uncancellable_operation_behind_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let tool = Prepared::for_test(root.path().join("gh"), root.path().join("auth")).unwrap();
    crate::resource_lock::with_resource(&tool.config_dir, || {
        let error =
            with_profile::<()>(&tool, || panic!("a busy operation must not run")).unwrap_err();
        assert!(error.contains("Another GitHub operation"));
    });
    assert_eq!(with_profile(&tool, || Ok(7)).unwrap(), 7);
}

#[cfg(unix)]
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    tool: Prepared,
}

#[cfg(unix)]
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(
            workspace.join("main.tex"),
            b"source stays local until explicit Push",
        )
        .unwrap();
        let expected = setup::inspect(&workspace).unwrap();
        setup::apply(
            &workspace,
            setup::SetupRequest {
                expected,
                initial_branch: "main".into(),
                author_name: "Fixture User".into(),
                author_email: "fixture@example.invalid".into(),
                origin: String::new(),
            },
        )
        .unwrap();
        let program = root.path().join("gh");
        fs::write(&program, r#"#!/bin/sh
set -eu
case "$*" in
  'auth status --active --hostname github.com --json hosts') cat "$GH_CONFIG_DIR/status.json" ;;
  'api --hostname github.com --method GET user') cat "$GH_CONFIG_DIR/user.json" ;;
  'api --hostname github.com --method GET repos/fixture-user/paper-backup')
    if test -f "$GH_CONFIG_DIR/created.json"; then cat "$GH_CONFIG_DIR/created.json"; else printf '%s\n' 'gh: Not Found (HTTP 404)' >&2; exit 1; fi ;;
  'api --hostname github.com --method POST user/repos --raw-field name=paper-backup --field private=true --field auto_init=false')
    set -- "$GH_CONFIG_DIR/repositories/"*.json
    test -f "$1" || { printf '%s\n' 'intent missing' >&2; exit 2; }
    printf '%s\n' post >> "$GH_CONFIG_DIR/posts"
    cp "$GH_CONFIG_DIR/response.json" "$GH_CONFIG_DIR/created.json"
    if test -f "$GH_CONFIG_DIR/lose-ack"; then printf '%s\n' 'gh: acknowledgement lost (HTTP 502)' >&2; exit 1; fi
    cat "$GH_CONFIG_DIR/created.json" ;;
  *) printf '%s\n' 'unexpected request ghp_must_not_be_exposed' >&2; exit 2 ;;
esac
"#).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let tool = Prepared::for_test(program, root.path().join("auth")).unwrap();
        fs::write(
            tool.config_dir.join("hosts.yml"),
            "github.com:\n    user: fixture-user\n    git_protocol: https\n",
        )
        .unwrap();
        fs::write(tool.config_dir.join("status.json"), r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"fixture-user","tokenSource":"keyring","gitProtocol":"https"}]}}"#).unwrap();
        fs::write(
            tool.config_dir.join("user.json"),
            r#"{"id":123,"login":"fixture-user","type":"User"}"#,
        )
        .unwrap();
        fs::write(tool.config_dir.join("response.json"), r#"{"id":456,"name":"paper-backup","private":true,"owner":{"id":123,"login":"fixture-user"},"html_url":"https://github.com/fixture-user/paper-backup","clone_url":"https://github.com/fixture-user/paper-backup.git"}"#).unwrap();
        Self {
            _root: root,
            workspace,
            tool,
        }
    }

    fn request(&self) -> CreateRequest {
        CreateRequest {
            expected: setup::inspect(&self.workspace).unwrap(),
            account: Account {
                id: 123,
                login: "fixture-user".into(),
            },
            name: "paper-backup".into(),
            private: true,
        }
    }

    fn record(&self) -> CreationRecord {
        let path = fs::read_dir(self.tool.config_dir.join("repositories"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        read_record(&path).unwrap()
    }

    fn posts(&self) -> usize {
        fs::read_to_string(self.tool.config_dir.join("posts"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

#[cfg(unix)]
#[test]
fn lost_acknowledgement_recovers_the_durable_intent_without_another_post() {
    let fixture = Fixture::new();
    let request = fixture.request();
    fs::write(fixture.tool.config_dir.join("lose-ack"), b"yes").unwrap();
    let error = create_with_tool(
        &fixture.workspace,
        request.clone(),
        &fixture.tool,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("Check https://github.com/fixture-user/paper-backup"));
    assert!(!error.contains("ghp_"));
    assert_eq!(fixture.posts(), 1);
    assert!(fixture.record().receipt.is_none());
    assert!(
        setup::inspect(&fixture.workspace)
            .unwrap()
            .origin
            .is_empty()
    );
    fs::remove_file(fixture.tool.config_dir.join("lose-ack")).unwrap();
    let created = create_with_tool(
        &fixture.workspace,
        request,
        &fixture.tool,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(created.confirmed());
    assert!(created.snapshot.is_ok(), "{:?}", created.snapshot);
    assert_eq!(
        fixture.posts(),
        1,
        "recovery must GET instead of repeating the POST"
    );
    assert_eq!(fixture.record().receipt.unwrap().id, 456);
    assert_eq!(
        setup::inspect(&fixture.workspace).unwrap().origin,
        created.clone_url
    );
    assert!(
        !fixture.workspace.join(".git/refs/heads/main").exists(),
        "creation never commits or pushes source"
    );
    let config = fs::read_to_string(fixture.workspace.join(".git/config")).unwrap();
    assert!(config.contains("auth git-credential"));
    assert!(config.contains("username = fixture-user"));
    assert!(!config.contains("oauth_token"));
    assert!(!config.contains("ghp_"));
}

#[cfg(unix)]
#[test]
fn confirmed_creation_survives_local_failure_and_retries_only_its_connection() {
    let fixture = Fixture::new();
    fs::write(
        fixture.workspace.join(".git/config.lock"),
        b"another git operation",
    )
    .unwrap();
    let created = create_with_tool(
        &fixture.workspace,
        fixture.request(),
        &fixture.tool,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(created.confirmed());
    assert!(created.snapshot.is_err());
    assert_eq!(fixture.posts(), 1);
    assert_eq!(fixture.record().receipt.unwrap().id, 456);
    assert!(
        setup::inspect(&fixture.workspace)
            .unwrap()
            .origin
            .is_empty()
    );
    fs::remove_file(fixture.workspace.join(".git/config.lock")).unwrap();
    let snapshot = setup::inspect(&fixture.workspace).unwrap();
    let connected = attach_with_tool(
        &fixture.workspace,
        snapshot,
        &created,
        &fixture.tool,
        &AtomicBool::new(false),
    )
    .unwrap();
    let (connected, authoritative) = connected.into_parts();
    assert_eq!(connected.origin, created.clone_url);
    assert!(authoritative.confirmed());
    assert_eq!(authoritative.repository_id(), Some(456));
    assert_eq!(authoritative.snapshot.as_ref().unwrap(), &connected);
    assert_eq!(fixture.posts(), 1);
    let cancelled = attach_with_tool(
        &fixture.workspace,
        connected,
        &created,
        &fixture.tool,
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert!(cancelled.contains("cancelled"));
}

#[cfg(unix)]
#[test]
fn connecting_an_unconfirmed_intent_retains_the_remote_identity_for_later_replacement_checks() {
    let fixture = Fixture::new();
    fs::write(fixture.tool.config_dir.join("lose-ack"), b"yes").unwrap();
    assert!(
        create_with_tool(
            &fixture.workspace,
            fixture.request(),
            &fixture.tool,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    let requested = from_record(fixture.record());
    assert!(!requested.confirmed());
    assert_eq!(requested.repository_id(), None);
    let expected = setup::inspect(&fixture.workspace).unwrap();
    let attached = attach_with_tool(
        &fixture.workspace,
        expected,
        &requested,
        &fixture.tool,
        &AtomicBool::new(false),
    )
    .unwrap();
    let (snapshot, confirmed) = attached.into_parts();
    assert!(confirmed.confirmed());
    assert_eq!(confirmed.repository_id(), Some(456));
    assert_eq!(confirmed.snapshot.as_ref().unwrap(), &snapshot);
    assert_eq!(
        fixture.posts(),
        1,
        "connection recovery must never send another creation request"
    );

    let path = fixture.tool.config_dir.join("created.json");
    let mut replacement: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    replacement["id"] = serde_json::json!(999);
    fs::write(path, serde_json::to_vec(&replacement).unwrap()).unwrap();
    let error = verify_repository(
        &fixture.tool,
        &snapshot,
        &confirmed,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("replaced"));
}

#[cfg(unix)]
#[test]
fn changed_account_or_unwritable_intent_cannot_create_a_remote() {
    let fixture = Fixture::new();
    let mut changed = fixture.request();
    changed.account.id = 999;
    assert!(
        create_with_tool(
            &fixture.workspace,
            changed,
            &fixture.tool,
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .contains("account changed")
    );
    assert_eq!(fixture.posts(), 0);
    fs::write(
        fixture.tool.config_dir.join("repositories"),
        b"not a directory",
    )
    .unwrap();
    assert!(
        create_with_tool(
            &fixture.workspace,
            fixture.request(),
            &fixture.tool,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(fixture.posts(), 0);
}

#[cfg(unix)]
#[test]
fn receipt_identity_and_urls_are_validated_before_recovery() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let response = fs::read(fixture.tool.config_dir.join("response.json")).unwrap();
    let created = parse_created(&request, &response).unwrap();
    let record = as_record(&created);
    write_record(&fixture.tool.config_dir, &record, true).unwrap();
    let path = record_path(&fixture.tool.config_dir, &record).unwrap();
    let mut corrupt = record.clone();
    corrupt.receipt.as_mut().unwrap().clone_url = "https://attacker.invalid/repository.git".into();
    fs::write(&path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    assert!(read_record(&path).is_err());
    let mut response: serde_json::Value = serde_json::from_slice(&response).unwrap();
    response["owner"]["id"] = serde_json::json!(999);
    assert!(parse_created(&request, &serde_json::to_vec(&response).unwrap()).is_err());
}
