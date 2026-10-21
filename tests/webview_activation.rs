//! Child construction must not trade background rendering for app activation.
#[path = "../vendor/wry/src/wkwebview/creation.rs"]
mod creation;

use syn::visit::Visit as _;

#[test]
fn embedded_preview_creation_never_activates_application() {
    assert!(!creation::should_activate_application(true));
    assert!(creation::should_activate_application(false));
}

#[derive(Default)]
struct ActivationCalls {
    depth: usize,
    guarded: usize,
    unguarded: usize,
    guards: usize,
}

fn activation_guard(expression: &syn::Expr) -> bool {
    let syn::Expr::Call(call) = expression else {
        return false;
    };
    let syn::Expr::Path(function) = call.func.as_ref() else {
        return false;
    };
    function
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        == ["creation", "should_activate_application"]
        && call.args.len() == 1
        && matches!(call.args.first(), Some(syn::Expr::Path(value)) if value.path.is_ident("is_child"))
}

impl<'ast> syn::visit::Visit<'ast> for ActivationCalls {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        if activation_guard(&node.cond) {
            self.guards += 1;
            self.depth += 1;
            self.visit_block(&node.then_branch);
            self.depth -= 1;
            if let Some((_, otherwise)) = &node.else_branch {
                self.visit_expr(otherwise);
            }
        } else {
            syn::visit::visit_expr_if(self, node);
        }
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(function) = node.func.as_ref() {
            let path = &function.path;
            if path.segments.len() == 2
                && path.segments[0].ident == "NSApplication"
                && matches!(
                    path.segments[1].ident.to_string().as_str(),
                    "activate" | "activateIgnoringOtherApps"
                )
            {
                if self.depth > 0 {
                    self.guarded += 1;
                } else {
                    self.unguarded += 1;
                }
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

#[test]
fn both_macos_activation_apis_obey_the_child_policy_regardless_of_focus_attributes() {
    let source = syn::parse_file(include_str!("../vendor/wry/src/wkwebview/mod.rs")).unwrap();
    let mut calls = ActivationCalls::default();
    calls.visit_file(&source);
    assert_eq!(calls.guards, 1);
    assert_eq!(
        calls.guarded, 2,
        "cover macOS 14 and older AppKit activation APIs"
    );
    assert_eq!(
        calls.unguarded, 0,
        "no child path may activate even with default focus attributes"
    );
}

#[test]
fn editor_child_creation_does_not_wait_for_focus_or_replace_the_parent_responder() {
    #[derive(Default)]
    struct Calls {
        methods: Vec<String>,
    }
    impl<'ast> syn::visit::Visit<'ast> for Calls {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            self.methods.push(node.method.to_string());
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    let source = syn::parse_file(include_str!("../src/app/native_views.rs")).unwrap();
    let mut creation_methods = None;
    for item in source.items {
        let syn::Item::Impl(implementation) = item else {
            continue;
        };
        for item in implementation.items {
            let syn::ImplItem::Fn(function) = item else {
                continue;
            };
            if function.sig.ident != "update_webview" {
                continue;
            }
            let mut calls = Calls::default();
            calls.visit_block(&function.block);
            if calls
                .methods
                .iter()
                .any(|method| method == "build_as_child")
            {
                assert!(creation_methods.replace(calls.methods).is_none());
            }
        }
    }
    let methods = creation_methods.expect("inspect the real native child constructor");
    for forbidden in ["has_focus", "focus_parent"] {
        assert!(
            !methods.iter().any(|method| method == forbidden),
            "child construction must not call {forbidden}"
        );
    }
    assert!(
        methods
            .iter()
            .any(|method| method == "with_background_throttling")
    );
}
