//! Package catalog presentation. Effects are returned to the owning popup.
use super::{success_color, warning_color};
use crate::{
    package_catalog::{PackageCatalogLoad, PackageRecord, PackageRootKind},
    theme,
};
use eframe::egui::{self, Align, Layout, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PackageFilter {
    All,
    Installed,
    Available,
    Updates,
}

impl PackageFilter {
    const ALL: [Self; 4] = [Self::All, Self::Installed, Self::Available, Self::Updates];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Installed => "Installed",
            Self::Available => "Available",
            Self::Updates => "Updates",
        }
    }

    fn allows(self, package: &PackageRecord) -> bool {
        match self {
            Self::All => true,
            Self::Installed => package.is_installed(),
            Self::Available => package.latest_available.is_some(),
            Self::Updates => package.has_update(),
        }
    }
}

#[derive(Default)]
pub(super) struct PackageBrowserAction {
    pub(super) copied: Option<String>,
    pub(super) open_link: Option<String>,
    pub(super) uninstall: Option<crate::package_catalog::PackageInstallation>,
}

pub(super) fn show_package_browser_ui(
    ui: &mut egui::Ui,
    query: &mut String,
    filter: &mut PackageFilter,
    load: Option<&PackageCatalogLoad>,
    loading: bool,
    action: &mut PackageBrowserAction,
) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Search package names, descriptions, authors, or versions")
                .desired_width(f32::INFINITY),
        );
    });
    ui.horizontal_wrapped(|ui| {
        theme::apply_compact_control_spacing(ui);
        for choice in PackageFilter::ALL {
            ui.selectable_value(filter, choice, choice.label());
        }
        if loading {
            ui.spinner();
            ui.label("Refreshing local and published packages…");
        }
    });
    ui.separator();

    let Some(load) = load else {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE.content);
            if loading {
                ui.spinner();
                ui.label("Inspecting Typst package directories and registry…");
            } else {
                ui.label(RichText::new("No package catalog has been loaded").weak());
            }
        });
        return;
    };

    if let Some(error) = &load.official_index_error {
        ui.colored_label(
            warning_color(ui.ctx()),
            format!("Published registry unavailable: {error}. Local packages are still shown."),
        );
    }
    if !load.warnings.is_empty() {
        egui::CollapsingHeader::new(format!(
            "{} local package scan warning{}",
            load.warnings.len(),
            if load.warnings.len() == 1 { "" } else { "s" }
        ))
        .show(ui, |ui| {
            for warning in &load.warnings {
                ui.label(format!("{}: {}", warning.path.display(), warning.message));
            }
        });
    }

    let packages = load
        .catalog
        .filtered(query)
        .into_iter()
        .filter(|package| filter.allows(package))
        .collect::<Vec<_>>();
    ui.label(
        RichText::new(format!(
            "{} package{}",
            packages.len(),
            if packages.len() == 1 { "" } else { "s" }
        ))
        .size(theme::TYPE.supporting)
        .weak(),
    );
    egui::ScrollArea::vertical()
        .id_salt("package-catalog-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for package in packages {
                ui.push_id((&package.namespace, &package.name), |ui| {
                    package_row(ui, package, loading, action);
                });
                ui.separator();
            }
        });
}

fn package_row(
    ui: &mut egui::Ui,
    package: &PackageRecord,
    loading: bool,
    action: &mut PackageBrowserAction,
) -> egui::Rect {
    let version = package
        .latest_available
        .or_else(|| package.latest_installed());
    let release = package.display_release();
    ui.scope(|ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(version.is_some(), egui::Button::new("Copy import"))
                    .clicked()
                    && let Some(version) = version
                {
                    action.copied = Some(format!(
                        "#import \"@{}/{}:{version}\": *",
                        package.namespace, package.name
                    ));
                }
                if let Some(website) = release.and_then(|release| {
                    release
                        .metadata
                        .homepage
                        .as_deref()
                        .or(release.metadata.repository.as_deref())
                }) && ui.button("Website").clicked()
                {
                    action.open_link = Some(website.to_owned());
                }
                let identity = version.map_or_else(
                    || format!("@{}/{}", package.namespace, package.name),
                    |version| format!("@{}/{}:{version}", package.namespace, package.name),
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(identity).monospace().strong())
                                .truncate()
                                .show_tooltip_when_elided(false),
                        );
                    },
                );
            });
        });
        if let Some(description) =
            release.and_then(|release| release.metadata.description.as_deref())
        {
            ui.label(description);
        }
        if let Some(installed) = package.latest_installed() {
            let label = if package.has_update() {
                format!(
                    "Installed versions · {installed} installed · {} available",
                    package.latest_available.unwrap()
                )
            } else {
                format!(
                    "Installed versions · {installed}{}",
                    if package.latest_available.is_none() {
                        " · Local"
                    } else {
                        ""
                    }
                )
            };
            let color = if package.has_update() {
                warning_color(ui.ctx())
            } else {
                success_color(ui.ctx())
            };
            egui::CollapsingHeader::new(RichText::new(label).color(color))
                .id_salt("installed-versions")
                .show(ui, |ui| {
                    for release in package.installed_releases() {
                        ui.label(
                            RichText::new(release.version.to_string())
                                .monospace()
                                .strong(),
                        );
                        for installation in &release.installations {
                            ui.horizontal_wrapped(|ui| {
                                let uninstall = ui.add_enabled(
                                    !loading,
                                    egui::Button::new(format!("Uninstall {}…", release.version)),
                                );
                                if uninstall.clicked() {
                                    action.uninstall = Some(installation.clone());
                                }
                                let kind = match installation.root.kind {
                                    PackageRootKind::Data => "data",
                                    PackageRootKind::Cache => "cache",
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "{}{kind}: {}",
                                        if installation.root.custom {
                                            "custom "
                                        } else {
                                            ""
                                        },
                                        installation.package_path.display()
                                    ))
                                    .monospace()
                                    .weak(),
                                );
                            });
                        }
                    }
                });
        } else {
            ui.label(
                RichText::new(if version.is_some() {
                    "Available"
                } else {
                    "Not installed"
                })
                .weak(),
            );
        }
    })
    .response
    .rect
}

#[cfg(test)]
mod tests {
    use super::{PackageBrowserAction, package_row};
    use crate::{
        package_catalog::{
            PackageInstallation, PackageMetadata, PackageRecord, PackageRelease, PackageRoot,
            PackageRootKind,
        },
        theme,
    };
    use eframe::egui;
    use egui_kittest::{Harness, kittest::Queryable as _};

    fn record() -> PackageRecord {
        let root = PackageRoot {
            path: "/private/packages".into(),
            kind: PackageRootKind::Data,
            custom: true,
        };
        let path = root.path.join("preview/cetz/0.2.0");
        let metadata = PackageMetadata {
            description: Some("Draw diagrams directly in Typst.".into()),
            homepage: Some("https://typst.app/universe/package/cetz".into()),
            ..Default::default()
        };
        PackageRecord {
            namespace: "preview".into(),
            name: "cetz".into(),
            latest_available: Some("0.3.0".parse().unwrap()),
            releases: vec![
                PackageRelease {
                    version: "0.3.0".parse().unwrap(),
                    available: true,
                    installations: vec![],
                    metadata: metadata.clone(),
                    updated_at: None,
                },
                PackageRelease {
                    version: "0.2.0".parse().unwrap(),
                    available: true,
                    installations: vec![PackageInstallation {
                        manifest_path: path.join("typst.toml"),
                        package_path: path,
                        root,
                    }],
                    metadata,
                    updated_at: None,
                },
            ],
        }
    }

    #[test]
    fn compact_package_row_keeps_actions_visible_and_installations_collapsed() {
        let package = record();
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(520.0, 420.0))
            .build_ui_state(
                move |ui, state: &mut (PackageBrowserAction, egui::Rect)| {
                    if !configured {
                        theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    state.1 = package_row(ui, &package, false, &mut state.0);
                },
                (PackageBrowserAction::default(), egui::Rect::NOTHING),
            );
        harness.run_steps(3);
        assert!(
            harness.state().1.height() < 110.0,
            "{:?}",
            harness.state().1
        );
        for label in ["Copy import", "Website"] {
            let rect = harness.get_by_label(label).rect();
            assert!(
                rect.left() >= 0.0 && rect.right() <= 520.0,
                "{label}: {rect:?}"
            );
        }
        let identity = harness
            .get_by_role_and_label(egui::accesskit::Role::Label, "@preview/cetz:0.3.0")
            .rect();
        let description = harness
            .get_by_role_and_label(
                egui::accesskit::Role::Label,
                "Draw diagrams directly in Typst.",
            )
            .rect();
        assert_eq!(
            identity.left(),
            description.left(),
            "{identity:?} {description:?}"
        );
        assert!(harness.query_by_label_contains("custom data:").is_none());
        assert!(harness.query_by_label("Uninstall 0.2.0…").is_none());
        harness.get_by_label("Copy import").click();
        harness.run();
        assert_eq!(
            harness.state().0.copied.as_deref(),
            Some("#import \"@preview/cetz:0.3.0\": *")
        );
        harness.get_by_label("Website").click();
        harness.run();
        assert_eq!(
            harness.state().0.open_link.as_deref(),
            Some("https://typst.app/universe/package/cetz")
        );
        harness.get_by_label_contains("Installed versions").click();
        harness.run_steps(10);
        assert!(harness.query_by_label_contains("custom data:").is_some());
        harness.get_by_label("Uninstall 0.2.0…").click();
        harness.run();
        assert_eq!(
            harness.state().0.uninstall.as_ref().unwrap().package_path,
            std::path::Path::new("/private/packages/preview/cetz/0.2.0")
        );
    }
}
