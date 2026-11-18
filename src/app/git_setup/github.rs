//! GitHub form state contains public account/repository metadata, never tokens.
use super::*;
use crate::github::{Account, CreatedRepository, Progress};

pub(super) struct Form {
    pub(super) selected: bool,
    pub(super) account: Option<Account>,
    pub(super) auth_error: Option<String>,
    pub(super) name: String,
    pub(super) private: bool,
    pub(super) sign_in: SignInPhase,
    pub(super) created: Option<CreatedRepository>,
    pub(super) opened_url: Option<String>,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fixture: bool,
}

#[derive(Default)]
pub(super) enum SignInPhase {
    #[default]
    Idle,
    Preparing,
    WaitingForBrowser,
    DeviceCode {
        code: String,
        uri: String,
    },
    Cancelling,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            selected: false,
            account: None,
            auth_error: None,
            name: String::new(),
            private: true,
            sign_in: SignInPhase::Idle,
            created: None,
            opened_url: None,
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            fixture: false,
        }
    }
}

impl Form {
    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub(super) fn stage(&self) -> &'static str {
        match self.sign_in {
            SignInPhase::Idle => "idle",
            SignInPhase::Preparing => "preparing",
            SignInPhase::WaitingForBrowser | SignInPhase::DeviceCode { .. } => "waiting",
            SignInPhase::Cancelling => "cancelling",
        }
    }

    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub(super) fn code(&self) -> Option<(&String, &String)> {
        match &self.sign_in {
            SignInPhase::DeviceCode { code, uri } => Some((code, uri)),
            _ => None,
        }
    }

    pub(super) fn begin_sign_in(&mut self) {
        self.account = None;
        self.auth_error = None;
        self.opened_url = None;
        self.sign_in = SignInPhase::Preparing;
    }

    pub(super) fn finish_sign_in(&mut self) {
        self.sign_in = SignInPhase::Idle;
    }

    pub(super) fn cancel_sign_in(&mut self) {
        self.sign_in = SignInPhase::Cancelling;
    }

    pub(super) fn progress(&mut self, progress: Progress) {
        if matches!(self.sign_in, SignInPhase::Idle | SignInPhase::Cancelling) {
            return;
        }
        match progress {
            Progress::Preparing => self.sign_in = SignInPhase::Preparing,
            Progress::DeviceCode {
                code,
                verification_uri,
            } => {
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                {
                    self.fixture = url::Url::parse(&verification_uri).is_ok_and(|uri| {
                        matches!(uri.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                    });
                }
                self.sign_in = SignInPhase::DeviceCode {
                    code,
                    uri: verification_uri,
                };
            }
            Progress::WaitingForBrowser => {
                if !matches!(self.sign_in, SignInPhase::DeviceCode { .. }) {
                    self.sign_in = SignInPhase::WaitingForBrowser;
                }
            }
        }
    }

    pub(super) fn ready(&self) -> bool {
        self.account.is_some() && !self.name.trim().is_empty() && self.created.is_none()
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<Action> {
        let mut action = None;
        ui.add_enabled_ui(!busy && self.created.is_none(), |ui| {
            let name = ui.label("GitHub repository name");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .id_salt("github-repository-name")
                    .desired_width(f32::INFINITY),
            );
            let response = response.labelled_by(name.id);
            observe("name", &response);
            let private = ui.checkbox(&mut self.private, "Private repository");
            observe("private", &private);
        });
        if let Some(account) = &self.account {
            ui.label(format!("Signed in as {}", account.login));
        }
        if let Some(error) = &self.auth_error {
            ui.colored_label(error_color(ui.ctx()), error);
        }
        if let Some(created) = &self.created {
            ui.hyperlink_to(&created.html_url, &created.html_url);
            match &created.snapshot {
                Ok(_) => {
                    ui.colored_label(
                        theme::palette(ui.ctx()).success,
                        "GitHub repository created and connected.",
                    );
                }
                Err(error) => {
                    ui.colored_label(error_color(ui.ctx()), error);
                    let attach = ui.add_enabled(!busy, egui::Button::new("Retry connection"));
                    observe("retry_attach", &attach);
                    if attach.clicked() {
                        action = Some(Action::AttachGitHub);
                    }
                    if !created.confirmed() {
                        let review =
                            ui.add_enabled(!busy, egui::Button::new("Review new repository"));
                        observe("review_new", &review);
                        if review.clicked() {
                            action = Some(Action::ForgetGitHub);
                        }
                    }
                }
            }
        }
        if matches!(self.sign_in, SignInPhase::Preparing) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing GitHub sign-in…");
            });
        }
        if let SignInPhase::DeviceCode { code, uri } = &self.sign_in {
            ui.horizontal(|ui| {
                ui.monospace(code);
                let copy = ui.button("Copy code");
                observe("code_copy", &copy);
                if copy.clicked() {
                    ui.ctx().copy_text(code.clone());
                }
                let open = ui.button("Open GitHub");
                observe("open", &open);
                if open.clicked() {
                    self.opened_url = Some(uri.clone());
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    let suppressed = self.fixture && crate::desktop_test::enabled();
                    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                    let suppressed = false;
                    if !suppressed {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(uri));
                    }
                }
            });
            ui.label("Enter this code in the browser and approve GitHub CLI.");
        }
        if !matches!(self.sign_in, SignInPhase::Idle) {
            let cancel = ui.add_enabled(
                !matches!(self.sign_in, SignInPhase::Cancelling),
                egui::Button::new("Cancel sign-in"),
            );
            observe("cancel", &cancel);
            if cancel.clicked() {
                action = Some(Action::CancelGitHub);
            }
        } else {
            let sign_in = ui
                .add_enabled(
                    !busy,
                    egui::Button::new(if self.account.is_some() {
                        "Sign in again"
                    } else {
                        "Sign in to GitHub"
                    }),
                )
                .on_hover_text(
                    "Uses the official GitHub CLI helper and your system credential store.",
                );
            observe("sign_in", &sign_in);
            if sign_in.clicked() {
                action = Some(Action::SignInGitHub);
            }
        }
        action
    }
}

fn observe(id: &str, response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("github.{id}"), response);
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = (id, response);
}

pub(super) fn repository_name(label: &str) -> String {
    let mut name = String::new();
    for character in label.chars().take(100) {
        let character = if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        {
            character
        } else {
            '-'
        };
        if character != '-' || !name.ends_with('-') {
            name.push(character);
        }
    }
    let name = name.trim_matches(['.', '-']);
    if name.is_empty() {
        "paper".into()
    } else {
        name.into()
    }
}
