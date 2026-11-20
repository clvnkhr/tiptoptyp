//! Ordinary integration contracts share one libtest executable.
//! Native AppKit/window event-loop harnesses remain separate main-thread targets.

#[path = "../build_support/tool_versions.rs"]
mod tool_versions;

#[path = "contracts/architecture_boundaries.rs"]
mod architecture_boundaries;
#[path = "contracts/build_identity.rs"]
mod build_identity;
#[path = "contracts/build_identity_inputs.rs"]
mod build_identity_inputs;
#[path = "contracts/builtin_themes.rs"]
mod builtin_themes;
#[path = "contracts/harper_archive.rs"]
mod harper_archive;
#[path = "contracts/install_macos.rs"]
mod install_macos;
#[path = "contracts/launch_cli.rs"]
mod launch_cli;
#[path = "contracts/locked_versions.rs"]
mod locked_versions;
#[path = "contracts/macos_file_drop.rs"]
mod macos_file_drop;
#[path = "contracts/native_clipboard.rs"]
mod native_clipboard;
#[path = "contracts/package_resources.rs"]
mod package_resources;
#[path = "contracts/startup_configuration.rs"]
mod startup_configuration;
#[path = "contracts/sublime_theme.rs"]
mod sublime_theme;
#[path = "contracts/theme_gallery_manifest.rs"]
mod theme_gallery_manifest;
#[path = "contracts/tinymist_configuration_contract.rs"]
mod tinymist_configuration_contract;
#[path = "contracts/toolchain_manifest.rs"]
mod toolchain_manifest;
#[path = "contracts/webview_activation.rs"]
mod webview_activation;
#[path = "contracts/webview_ipc_scope.rs"]
mod webview_ipc_scope;
