//! One copper graph (docs/copper-graph.md): only the connectivity module
//! (`crates/editor-core/src/layout/connectivity`) decides whether copper
//! touches or joins. Everything else asks it.
//!
//! Joining is "same net, and the copper meets". Clearance is the opposite
//! question, "different nets, and the copper comes closer than a rule",
//! so this check looks for the first shape and leaves the second alone.
//! It parses the workspace's crates and apps and rejects, outside the
//! module:
//!
//! - the module's joining tolerances by name (`JOIN_EPS`, `JOIN_REACH_MM`);
//! - a function that both compares a net for equality (`a.net == b`) and
//!   tests copper for contact: a comparison with a contact tolerance (a
//!   float literal from 1e-8 up to 0.001 mm, such as `1e-6`), or a
//!   containment or touch predicate (`touches`, `track_contains`,
//!   `polygon_contains`). Smaller literals (`1e-9`, `1e-12`) test float
//!   identity, such as finding a track a step just wrote by its end point
//!   or skipping a degenerate segment, not whether copper meets.
//!
//! A clearance test compares nets for inequality and gaps against a rule,
//! so it passes. Counted, reasoned exceptions name functions that match
//! the shape but decide no connectivity. Test code (`#[cfg(test)]` items
//! and `*tests.rs` files) is not checked.
use crate::{Result, root};
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};

/// The module, relative to the workspace root.
const MODULE: &[&str] = &[
    "crates/editor-core/src/layout/connectivity.rs",
    "crates/editor-core/src/layout/connectivity/",
];

/// Names only the module may use: its joining tolerances.
const OWNED: &[&str] = &["JOIN_EPS", "JOIN_REACH_MM"];

/// Calls that test whether a point or shape lies on copper.
const CONTACT: &[&str] = &["touches", "track_contains", "polygon_contains"];

/// The literals read as a contact tolerance, in mm: below them float
/// identity, above them a design value.
const TOLERANCE: std::ops::Range<f64> = 1e-8..1e-3;

/// Functions that match the shape but decide no connectivity: (file,
/// function, why). A stale entry fails the check.
const EXCEPTIONS: &[(&str, &str, &str)] = &[];

/// What one function does.
#[derive(Default)]
struct Body {
    net_equal: bool,
    contact: bool,
}

#[derive(Default)]
struct Finder {
    /// Functions that look like joining code, by name and line.
    functions: Vec<(String, usize)>,
    /// Uses of the module's own names, by name and line.
    owned: Vec<(String, usize)>,
    /// The innermost function being read.
    stack: Vec<(String, usize, Body)>,
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>()
                .is_ok_and(|m| m.path().is_ident("test"))
    })
}

fn line(span: proc_macro2::Span) -> usize {
    span.start().line
}

/// `expr` without references, dereferences, parentheses, `Some(..)` and
/// `.copied()` / `.clone()` around it.
fn bare(expr: &syn::Expr) -> &syn::Expr {
    match expr {
        syn::Expr::Reference(r) => bare(&r.expr),
        syn::Expr::Paren(p) => bare(&p.expr),
        syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => bare(&u.expr),
        syn::Expr::Call(c) if c.args.len() == 1 => match (&*c.func, c.args.first()) {
            (syn::Expr::Path(p), Some(arg)) if p.path.is_ident("Some") => bare(arg),
            _ => expr,
        },
        syn::Expr::MethodCall(m)
            if m.args.is_empty() && matches!(m.method.to_string().as_str(), "copied" | "clone") =>
        {
            bare(&m.receiver)
        }
        _ => expr,
    }
}

/// Whether `expr` reads a net: a field `net`, a variable `net`, or `.net()`.
fn is_net(expr: &syn::Expr) -> bool {
    match bare(expr) {
        syn::Expr::Field(f) => matches!(&f.member, syn::Member::Named(n) if n == "net"),
        syn::Expr::Path(p) => p.path.is_ident("net"),
        syn::Expr::MethodCall(m) => m.method == "net" && m.args.is_empty(),
        _ => false,
    }
}

/// Whether `expr` is a float literal read as a tolerance.
fn is_tolerance(expr: &syn::Expr) -> bool {
    let syn::Expr::Lit(lit) = bare(expr) else {
        return false;
    };
    let syn::Lit::Float(f) = &lit.lit else {
        return false;
    };
    f.base10_parse::<f64>()
        .is_ok_and(|v| TOLERANCE.contains(&v))
}

impl Finder {
    fn enter(&mut self, name: String, at: usize) {
        self.stack.push((name, at, Body::default()));
    }

    fn leave(&mut self) {
        if let Some((name, at, body)) = self.stack.pop()
            && body.net_equal
            && body.contact
        {
            self.functions.push((name, at));
        }
    }

    fn body(&mut self) -> Option<&mut Body> {
        self.stack.last_mut().map(|(_, _, b)| b)
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            self.enter(item.sig.ident.to_string(), line(item.sig.ident.span()));
            visit::visit_item_fn(self, item);
            self.leave();
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            self.enter(item.sig.ident.to_string(), line(item.sig.ident.span()));
            visit::visit_impl_item_fn(self, item);
            self.leave();
        }
    }

    fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
        let (left, right) = (&*expr.left, &*expr.right);
        match expr.op {
            syn::BinOp::Eq(_) if is_net(left) || is_net(right) => {
                if let Some(body) = self.body() {
                    body.net_equal = true;
                }
            }
            syn::BinOp::Lt(_) | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_)
                if is_tolerance(left) || is_tolerance(right) =>
            {
                if let Some(body) = self.body() {
                    body.contact = true;
                }
            }
            _ => {}
        }
        visit::visit_expr_binary(self, expr);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if CONTACT.contains(&call.method.to_string().as_str())
            && let Some(body) = self.body()
        {
            body.contact = true;
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*call.func
            && p.path
                .segments
                .last()
                .is_some_and(|s| CONTACT.contains(&s.ident.to_string().as_str()))
            && let Some(body) = self.body()
        {
            body.contact = true;
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        let name = ident.to_string();
        if OWNED.contains(&name.as_str()) {
            self.owned.push((name, line(ident.span())));
        }
    }
}

fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// What one source file does outside the module: functions that look
/// like joining code, and uses of the module's own names.
type Found = (Vec<(String, usize)>, Vec<(String, usize)>);

fn scan(source: &str) -> std::result::Result<Found, String> {
    let syntax = syn::parse_file(source).map_err(|e| e.to_string())?;
    let mut finder = Finder::default();
    finder.visit_file(&syntax);
    Ok((finder.functions, finder.owned))
}

/// Every source file of the workspace's crates and apps, relative to the
/// root, that is not test code or the module itself.
fn sources(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for group in ["crates", "apps"] {
        for package in std::fs::read_dir(root.join(group))? {
            let src = package?.path().join("src");
            if src.is_dir() {
                rust_files(&src, &mut files)?;
            }
        }
    }
    let mut out = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let test = relative.ends_with("tests.rs") || relative.contains("/tests/");
        let module = MODULE.iter().any(|m| relative.starts_with(m));
        if !test && !module {
            out.push((relative, file));
        }
    }
    out.sort();
    Ok(out)
}

pub fn check() -> Result<()> {
    let root = root()?;
    let mut violations = Vec::new();
    let mut used = vec![false; EXCEPTIONS.len()];
    for (relative, file) in sources(&root)? {
        let source = std::fs::read_to_string(&file)?;
        let (functions, owned) =
            scan(&source).map_err(|e| format!("{relative}: cannot parse: {e}"))?;
        for (name, at) in owned {
            violations.push(format!(
                "{relative}:{at}: {name} is the copper graph's joining tolerance; \
                 ask layout::connectivity instead"
            ));
        }
        for (name, at) in functions {
            match EXCEPTIONS
                .iter()
                .position(|(f, n, _)| *f == relative && *n == name)
            {
                Some(k) => {
                    if let Some(slot) = used.get_mut(k) {
                        *slot = true;
                    }
                }
                None => violations.push(format!(
                    "{relative}:{at}: `{name}` compares nets for equality and tests copper \
                     for contact: whether copper joins is decided only in \
                     layout::connectivity (CopperGraph::joints_on, joints_of_via, \
                     islands_at, islands_joined); ask it"
                )),
            }
        }
    }
    for ((file, name, _), used) in EXCEPTIONS.iter().zip(used) {
        if !used {
            violations.push(format!(
                "{file}: the exception for `{name}` matches nothing; remove it"
            ));
        }
    }
    if violations.is_empty() {
        eprintln!("Connectivity in one module passed");
        Ok(())
    } else {
        Err(violations.join("\n").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        scan(source)
            .map(|(f, _)| f.into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default()
    }

    #[test]
    fn joining_code_is_found_and_clearance_is_not() {
        // Track ends of one net meeting within a micrometre.
        assert_eq!(
            names(
                "fn ends(t: &Track, u: &Track) -> bool { \
                 t.net == u.net && distance(t.end, u.start) < 1e-6 }"
            ),
            ["ends"]
        );
        // A point on a track of the net.
        assert_eq!(
            names(
                "fn on(p: &Project, net: Id, at: Point) -> bool { \
                 p.tracks.iter().any(|t| t.net == net && layout::track_contains(t, at, 0.0)) }"
            ),
            ["on"]
        );
        // Clearance: other nets, against a rule.
        assert!(
            names(
                "fn clear(a: &Ring, b: &Ring, need: f64) -> bool { \
                 a.net != b.net && a.ring.closer_than(&b.ring, need) }"
            )
            .is_empty()
        );
        // Picking: what is under the pointer, whatever its net.
        assert!(
            names("fn pick(t: &Track, at: Point) -> Option<Id> { track_contains(t, at, 0.0).then_some(t.net) }")
                .is_empty()
        );
        // Float identity: the track a step just wrote, found by its end.
        assert!(
            names("fn last(t: &Track, net: Id, end: Point) -> bool { t.net == net && distance(t.end, end) < 1e-9 }")
                .is_empty()
        );
        // Test code is skipped.
        assert!(
            names("#[cfg(test)] mod tests { fn t(a: &T, b: &T) -> bool { a.net == b.net && a.x < 1e-9 } }")
                .is_empty()
        );
    }

    #[test]
    fn the_joining_tolerance_is_the_modules_alone() {
        let owned = scan("fn f() -> f64 { connectivity::JOIN_EPS }")
            .map(|(_, o)| o.len())
            .unwrap_or_default();
        assert_eq!(owned, 1);
    }

    #[test]
    fn every_exception_says_why() {
        for (file, name, why) in EXCEPTIONS {
            assert!(!name.is_empty() && !why.is_empty(), "{file}");
        }
    }
}
