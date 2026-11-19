//! Login continuation uses committed routes, never login-page DOM or credentials.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use crate::overleaf::ProjectUrl;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SignIn {
    #[default]
    None,
    Required,
    ResumeProject,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Recovery {
    #[default]
    Watching,
    AwaitingReturn,
    Cancelled,
}
#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl Recovery {
    pub(super) fn observe(&mut self, project: Option<&ProjectUrl>, candidate: &str) -> SignIn {
        let Some(project) = project else {
            return SignIn::None;
        };
        let Ok(url) = url::Url::parse(candidate) else {
            return SignIn::None;
        };
        if url.origin().ascii_serialization() != project.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return SignIn::None;
        }
        let path = url.path().strip_suffix('/').unwrap_or(url.path());
        if path == "/login" {
            if *self != Self::Cancelled {
                *self = Self::AwaitingReturn;
            }
            return SignIn::Required;
        }
        if *self == Self::AwaitingReturn
            && (path == "/project" || path.strip_prefix("/project/") == Some(project.id()))
        {
            // Classification is idempotent while committed redirects coalesce.
            // The controller owns the single actual return attempt across
            // native WebView replacement and clears it only after hydration.
            return SignIn::ResumeProject;
        }
        SignIn::None
    }
    pub(super) fn cancel(&mut self) {
        *self = Self::Cancelled;
    }
    /// A verified, completely hydrated project retires its observed login.
    pub(super) fn ready(&mut self) {
        if *self != Self::Cancelled {
            *self = Self::Watching;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project() -> ProjectUrl {
        ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap()
    }
    #[test]
    fn login_return_is_same_origin_route_based_and_requires_login_since_hydration() {
        let project = project();
        let mut recovery = Recovery::default();
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(
                Some(&project),
                "https://www.overleaf.com/login/?next=%2Fproject"
            ),
            SignIn::Required
        );
        for unrelated in [
            "https://identity.example/login",
            "https://identity.example/project",
            "https://www.overleaf.com.evil.example/project",
            "http://www.overleaf.com/project",
            "https://www.overleaf.com:444/project",
            "https://user:secret@www.overleaf.com/project",
        ] {
            assert_eq!(
                recovery.observe(Some(&project), unrelated),
                SignIn::None,
                "{unrelated}"
            );
        }
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project/"),
            SignIn::ResumeProject
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/login"),
            SignIn::Required
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::ResumeProject
        );
        assert_eq!(
            recovery.observe(Some(&project), project.url()),
            SignIn::ResumeProject
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::ResumeProject
        );
        recovery.ready();
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/login"),
            SignIn::Required
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::ResumeProject
        );
    }
    #[test]
    fn ordinary_browsing_and_explicit_navigation_never_choose_a_project() {
        let project = project();
        let mut recovery = Recovery::default();
        assert_eq!(
            recovery.observe(None, "https://www.overleaf.com/login"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(None, "https://www.overleaf.com/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/login"),
            SignIn::Required
        );
        recovery.cancel();
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::None
        );
        recovery.ready();
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/login"),
            SignIn::Required
        );
        assert_eq!(
            recovery.observe(Some(&project), "https://www.overleaf.com/project"),
            SignIn::None
        );
    }
    #[test]
    fn explicit_self_hosted_project_returns_only_to_its_exact_server() {
        let project =
            ProjectUrl::parse("http://127.0.0.1:43210/project/0123456789abcdef01234567").unwrap();
        let mut recovery = Recovery::default();
        assert_eq!(
            recovery.observe(Some(&project), "http://127.0.0.1:43210/login"),
            SignIn::Required
        );
        assert_eq!(
            recovery.observe(Some(&project), "http://localhost:43210/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), "http://127.0.0.1:43211/project"),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), "http://127.0.0.1:43210/project"),
            SignIn::ResumeProject
        );
    }

    #[test]
    fn exact_project_login_return_survives_unconsumed_committed_redirects() {
        let project = project();
        for dashboard in [false, true] {
            let mut navigation = super::super::ProjectNavigation::new(project.url()).unwrap();
            navigation.started(project.url());
            navigation.pending.take();
            navigation.started("https://www.overleaf.com/login");
            if dashboard {
                navigation.started("https://www.overleaf.com/project");
            }
            let target = format!("{}?doc=abcdef0123456789abcdef01#editor", project.url());
            navigation.started(&target);
            let pending = navigation.pending.take().expect("latest committed route");
            assert_eq!(pending.url, target);
            assert_eq!(pending.project, Some(project.clone()));
            assert_eq!(
                pending.sign_in,
                SignIn::ResumeProject,
                "dashboard={dashboard}"
            );
        }
    }

    #[test]
    fn login_return_never_substitutes_the_bound_project_for_another_selected_project() {
        let project = project();
        let mut recovery = Recovery::default();
        recovery.observe(Some(&project), "https://www.overleaf.com/login");
        assert_eq!(
            recovery.observe(
                Some(&project),
                "https://www.overleaf.com/project/abcdef0123456789abcdef01"
            ),
            SignIn::None
        );
        assert_eq!(
            recovery.observe(Some(&project), project.url()),
            SignIn::ResumeProject
        );
    }
}
