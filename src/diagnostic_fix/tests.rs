use super::*;

#[test]
fn explicit_download_rejects_invalid_or_canceled_setup_without_writing_files() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("not-created");
    let cancel = AtomicBool::new(true);
    assert!(download(&cache, &cancel).is_err());
    assert!(!cache.exists());
    cancel.store(false, Ordering::Release);
    assert!(download(Path::new("relative-cache"), &cancel).is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

fn reply(content: serde_json::Value, finish: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"choices": [{"finish_reason": finish,
        "message": {"content": serde_json::to_string(&content).unwrap()}}]}))
    .unwrap()
}

#[test]
fn structured_output_requires_one_complete_bounded_replacement() {
    assert_eq!(
        parse_completion(&reply(
            serde_json::json!({"replacement":"#heading[Title]"}),
            "stop"
        ))
        .unwrap(),
        "#heading[Title]"
    );
    assert_eq!(
        parse_completion(&reply(serde_json::json!({"replacement":""}), "stop")).unwrap(),
        ""
    );
    for bytes in [
        reply(serde_json::json!({"replacement":"x"}), "length"),
        reply(
            serde_json::json!({"replacement":"x","explanation":"ignored"}),
            "stop",
        ),
        reply(serde_json::json!({"replacement":null}), "stop"),
        reply(serde_json::json!({"replacement":"\0"}), "stop"),
        reply(
            serde_json::json!({"replacement":"x".repeat(MAX_REPLACEMENT_BYTES+1)}),
            "stop",
        ),
        br#"{"choices":[]}"#.to_vec(),
        vec![b' '; MAX_REPLY_BYTES + 1],
    ] {
        assert!(parse_completion(&bytes).is_err());
    }
}

#[test]
fn generation_request_is_local_bounded_and_constrained_without_tools() {
    let request: serde_json::Value =
        serde_json::from_slice(&completion_request("source data")).unwrap();
    assert_eq!(request["messages"][0]["content"], SYSTEM);
    assert_eq!(request["messages"][1]["content"], "source data");
    assert_eq!(request["temperature"], 0.0);
    assert_eq!(request["stream"], false);
    assert_eq!(
        request["response_format"]["json_schema"]["schema"]["additionalProperties"],
        false
    );
    assert!(request.get("tools").is_none());
    assert!(validate_request(Path::new("relative"), 4, "source").is_err());
    assert!(validate_request(Path::new("/absolute"), 9, "source").is_err());
    assert!(
        validate_request(Path::new("/absolute"), 4, &"x".repeat(MAX_PROMPT_BYTES + 1)).is_err()
    );
}

#[test]
fn shared_client_is_inert_and_lock_waits_are_cancellable() {
    let first = shared_client();
    let second = shared_client();
    assert!(Arc::ptr_eq(&first, &second));
    let guard = first.lock().unwrap();
    assert!(guard.loaded.is_none());
    let cancel = AtomicBool::new(true);
    assert!(lock_client(&first, &cancel).is_err());
}

#[test]
fn truncated_http_response_and_extra_choices_cannot_become_a_review() {
    assert!(parse_completion(br#"{"choices":[{"finish_reason":"stop","message":{"content":"{\"replacement\":\"partial"}}]}"#).is_err());
    let one: serde_json::Value =
        serde_json::from_slice(&reply(serde_json::json!({"replacement":"x"}), "stop")).unwrap();
    let bytes =
        serde_json::to_vec(&serde_json::json!({"choices":[one["choices"][0],one["choices"][0]]}))
            .unwrap();
    assert!(parse_completion(&bytes).is_err());
}

#[cfg(unix)]
#[test]
fn parent_pipe_eof_terminates_and_reaps_only_the_owned_model_child() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("model-pid");
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "echo $$ > \"$1\"; exec /bin/sleep 60",
            "model-fixture",
        ])
        .arg(&pid_file);
    let mut child = spawn_guarded(&command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "fixture did not start");
        thread::sleep(Duration::from_millis(5));
    };
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok());
    drop(child.child_mut().stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "parent EOF did not stop the model"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}
