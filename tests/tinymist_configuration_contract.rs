#[path = "../build_support/tinymist_configuration.rs"]
mod configuration;
#[path = "../build_support/tool_versions.rs"]
mod tool_versions;

use serde_json::json;

#[test]
fn bundled_server_must_have_an_explicitly_reviewed_configuration_contract() {
    let versions = tool_versions::versions(include_str!("../toolchain/manifest.tsv")).unwrap();
    assert!(configuration::supports_version(versions["tinymist"]));
    for unreviewed in ["0.15.2", "0.15.13", "0.16.0", "", "0.15.12-custom"] {
        assert!(!configuration::supports_version(unreviewed), "{unreviewed}");
    }
}

#[test]
fn built_in_defaults_satisfy_the_same_contract_used_by_saved_settings() {
    let defaults = configuration::defaults();
    configuration::validate(&defaults).unwrap();
    assert_eq!(defaults["formatterProseWrap"], "none");
    assert_eq!(defaults["exportPdf"], "never");
    assert_eq!(defaults["lint"]["enabled"], false);
}

#[test]
fn retired_boolean_modes_cannot_pass_the_build_or_runtime_validator() {
    for retired in [json!(false), json!(true), json!(null), json!("false")] {
        let mut defaults = configuration::defaults();
        defaults["formatterProseWrap"] = retired;
        assert!(
            configuration::validate(&defaults)
                .unwrap_err()
                .starts_with("formatterProseWrap:"),
        );
    }
    for mode in ["none", "fill", "sentence"] {
        configuration::validate(&json!({"formatterProseWrap":mode})).unwrap();
    }
}

#[test]
fn custom_keys_do_not_override_app_owned_transport_or_entry_arguments() {
    configuration::validate(&json!({"futureCustomServerOption":{"enabled":true}})).unwrap();
    for managed in ["preview", "compileStatus", "customizedShowDocument"] {
        let mut configuration = json!({});
        configuration[managed] = json!({});
        assert!(
            configuration::validate(&configuration)
                .unwrap_err()
                .contains("managed by the app")
        );
    }
    assert!(configuration::validate(&json!({"typstExtraArgs":["other.typ"]})).is_err());
    assert_eq!(
        configuration::compiler_input_arguments(&json!(["--input", "mode=draft"])).unwrap(),
        vec![json!("--input=mode=draft")],
    );
}
