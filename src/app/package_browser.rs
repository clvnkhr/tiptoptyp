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
                let version = package
                    .latest_available
                    .or_else(|| package.latest_installed());
                let release = package.display_release();
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        let identity = version.map_or_else(
                            || format!("@{}/{}", package.namespace, package.name),
                            |version| format!("@{}/{}:{version}", package.namespace, package.name),
                        );
                        ui.label(RichText::new(&identity).monospace().strong());
                        if package.is_installed() {
                            ui.label(RichText::new("Installed").color(success_color(ui.ctx())));
                        }
                        if package.latest_available.is_some() {
                            ui.label(RichText::new("Published").weak());
                        }
                        if package.has_update() {
                            ui.label(
                                RichText::new("Update available").color(warning_color(ui.ctx())),
                            );
                        }
                        if let Some(website) = release.and_then(|release| {
                            release
                                .metadata
                                .homepage
                                .as_deref()
                                .or(release.metadata.repository.as_deref())
                        }) && crate::app::icons::action_button(ui, "Website").clicked()
                        {
                            action.open_link = Some(website.to_owned());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if crate::app::icons::action_button_enabled(
                                ui,
                                version.is_some(),
                                "Copy import",
                            )
                            .clicked()
                                && let Some(version) = version
                            {
                                action.copied = Some(format!(
                                    "#import \"@{}/{}:{version}\": *",
                                    package.namespace, package.name
                                ));
                            }
                        });
                    });
                    if let Some(description) =
                        release.and_then(|release| release.metadata.description.as_deref())
                    {
                        ui.label(description);
                    }
                    if let Some(installed) = package.latest_installed() {
                        ui.label(
                            RichText::new(format!("Latest local version: {installed}"))
                                .size(theme::TYPE.supporting)
                                .weak(),
                        );
                    }
                    for local_release in package.installed_releases() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("Installed {}", local_release.version))
                                    .monospace()
                                    .strong(),
                            );
                            if local_release.available {
                                ui.label(RichText::new("Published release").weak());
                            }
                        });
                        for installation in &local_release.installations {
                            if crate::app::icons::icon_button_enabled(
                                ui,
                                !loading,
                                crate::app::icons::UiIcon::Trash,
                                &format!("Uninstall {}…", local_release.version),
                            )
                            .clicked()
                            {
                                action.uninstall = Some(installation.clone());
                            }
                            let root_kind = match installation.root.kind {
                                PackageRootKind::Data => "data",
                                PackageRootKind::Cache => "cache",
                            };
                            let root_kind = if installation.root.custom {
                                format!("custom {root_kind}")
                            } else {
                                root_kind.to_owned()
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{root_kind}: {}",
                                    installation.package_path.display()
                                ))
                                .size(theme::TYPE.supporting)
                                .monospace()
                                .weak(),
                            );
                        }
                    }
                });
                ui.add_space(theme::SPACE.tight);
            }
        });
}
