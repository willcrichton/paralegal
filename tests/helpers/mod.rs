extern crate rustc_middle;
extern crate rustc_span;
use dfpp::{desc::Identifier, HashSet, Symbol};
use rustc_middle::mir;

use std::borrow::Cow;

lazy_static! {
    static ref CWD_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub static ref DFPP_INSTALLED: bool = install_dfpp();
}

pub fn with_current_directory<
    P: AsRef<std::path::Path>,
    A,
    F: std::panic::UnwindSafe + FnOnce() -> A,
>(
    p: P,
    f: F,
) -> std::io::Result<A> {
    let _guard = CWD_MUTEX.lock().unwrap();
    let current = std::env::current_dir()?;
    std::env::set_current_dir(p)?;
    let res = std::panic::catch_unwind(f);
    let set_dir = std::env::set_current_dir(current);
    match res {
        Ok(r) => set_dir.map(|()| r),
        Err(e) => std::panic::resume_unwind(e),
    }
}

pub fn cwd_and_use_rustc_in<
    P: AsRef<std::path::Path>,
    A,
    F: std::panic::UnwindSafe + FnOnce() -> A,
>(
    p: P,
    f: F,
) -> std::io::Result<A> {
    with_current_directory(p, || {
        rustc_span::create_default_session_if_not_set_then(|_| f())
    })
}

pub fn use_rustc<A, F: FnOnce() -> A>(f: F) -> A {
    rustc_span::create_default_session_if_not_set_then(|_| f())
}

pub fn install_dfpp() -> bool {
    std::process::Command::new("cargo")
        .arg("install")
        .arg("--locked")
        .arg("--offline")
        .arg("--path")
        .arg(".")
        .arg("--debug")
        .status()
        .unwrap()
        .success()
}

pub fn run_dfpp_with_graph_dump() -> bool {
    std::process::Command::new("cargo")
        .arg("dfpp")
        .arg("--dump-serialized-non-transitive-graph")
        .status()
        .unwrap()
        .success()
}

pub fn run_dfpp_with_flow_graph_dump() -> bool {
    std::process::Command::new("cargo")
        .arg("dfpp")
        .arg("--dump-serialized-flow-graph")
        .arg("--debug")
        .status()
        .unwrap()
        .success()
}

pub type SimpleMirBody = Vec<(mir::Location, String, HashSet<Symbol>)>;

use dfpp::foreign_serializers::SerializableNonTransitiveGraph;

pub struct G {
    pub graph: SerializableNonTransitiveGraph,
    pub body: SimpleMirBody,
}

impl G {
    fn predecessors(&self, n: mir::Location) -> impl Iterator<Item = &mir::Location> {
        self.graph.get(&n).into_iter().flat_map(move |r| {
            self.body
                .iter()
                .find(|t| t.0 == n)
                .unwrap()
                .2
                .iter()
                .flat_map(|p| r.row(*p))
        })
    }
    pub fn connects(&self, from: mir::Location, to: mir::Location) -> bool {
        let mut queue = vec![to];
        let mut seen = HashSet::new();
        while let Some(n) = queue.pop() {
            if seen.contains(&n) {
                continue;
            } else {
                seen.insert(n);
            }
            if n == from {
                return true;
            }
            queue.extend(self.predecessors(n))
        }
        false
    }
    pub fn connects_direct(&self, from: mir::Location, to: mir::Location) -> bool {
        self.predecessors(to).any(|l| *l == from)
    }

    pub fn function_call(&self, pattern: &str) -> mir::Location {
        self.body
            .iter()
            .find(|(_, s, _)| s.contains(pattern))
            .unwrap_or_else(|| panic!("Pattern {pattern} not found in {:?}", self.body))
            .0
    }
    pub fn from_file(s: Symbol) -> Self {
        let (body, graph) = dfpp::dbg::read_non_transitive_graph_and_body(s);
        Self { graph, body }
    }
    pub fn argument(&self, n: usize) -> mir::Location {
        self.body
            .iter()
            .find(|(_, s, _)| s == format!("Argument _{n}").as_str())
            .unwrap_or_else(|| panic!("Argument {n} not found in {:?}", self.body))
            .0
    }
}

use dfpp::desc::ProgramDescription;

pub trait HasGraph<'g> {
    fn graph(self) -> &'g ProgramDescription;
}

pub struct PreFrg(ProgramDescription);

impl<'g> HasGraph<'g> for &'g PreFrg {
    fn graph(self) -> &'g ProgramDescription {
        &self.0
    }
}

impl PreFrg {
    pub fn from_file_at(dir: &str) -> Self {
        use_rustc(|| {
            Self(
                serde_json::from_reader(
                    &mut std::fs::File::open(format!("{dir}/{}", dfpp::FLOW_GRAPH_OUT_NAME))
                        .unwrap(),
                )
                .unwrap(),
            )
        })
    }

    pub fn function(&self, name: &str) -> FnRef {
        FnRef {
            graph: self,
            ident: Identifier::from_str(name),
        }
    }

    pub fn ctrl(&self, name: &str) -> CtrlRef {
        let ident = Identifier::from_str(name);
        CtrlRef { graph: self, ident, ctrl: &self.0.controllers[&ident] }
    }
}

#[derive(Clone)]
pub struct CtrlRef<'g> {
    graph: &'g PreFrg,
    ident: Identifier,
    ctrl: &'g dfpp::desc::Ctrl,
}

impl <'g> PartialEq for CtrlRef<'g> {
    fn eq(&self, other: &Self) -> bool {
        self.ident == other.ident
    }
}

impl <'g> HasGraph<'g> for &CtrlRef<'g> {
    fn graph(self) -> &'g ProgramDescription {
        self.graph.graph()
    }
}

impl <'g> CtrlRef<'g> {
    pub fn call_sites(&'g self, fun: &'g FnRef<'g>) -> Vec<CallSiteRef<'g>> {
        let mut all: Vec<CallSiteRef<'g>> = self.ctrl
            .flow
            .0
            .values()
            .flat_map(|v| {
                v.iter().map(|sink| CallSiteRef {
                    function: fun,
                    call_site: &sink.function,
                    ctrl: Cow::Borrowed(self),
                })
            })
            .chain(
                self.ctrl.flow
                    .0
                    .keys()
                    .filter_map(dfpp::desc::DataSource::as_function_call)
                    .map(|f| CallSiteRef {
                        function: fun,
                        call_site: f,
                        ctrl: Cow::Borrowed(self),
                    }),
            )
            .filter(|ref_| ref_.function.ident == ref_.call_site.function)
            .collect();
        all.dedup_by_key(|r| r.call_site);
        all
    }
}

impl<'g> HasGraph<'g> for &FnRef<'g> {
    fn graph(self) -> &'g ProgramDescription {
        self.graph.graph()
    }
}

pub struct FnRef<'g> {
    graph: &'g PreFrg,
    ident: Identifier,
}

impl<'g> FnRef<'g> {
    fn graph(&self) -> &'g ProgramDescription {
        self.graph.graph()
    }

}

pub struct CallSiteRef<'g> {
    function: &'g FnRef<'g>,
    call_site: &'g dfpp::desc::CallSite,
    ctrl: Cow<'g, CtrlRef<'g>>,
}

impl<'g> PartialEq<dfpp::desc::CallSite> for CallSiteRef<'g> {
    fn eq(&self, other: &dfpp::desc::CallSite) -> bool {
        self.call_site == other
    }
}

impl<'g> CallSiteRef<'g> {
    pub fn input(&'g self) -> Vec<DataSinkRef<'g>> {
        let mut all: Vec<_> = self
            .ctrl
            .ctrl
            .flow
            .0
            .values()
            .flat_map(|s| s.iter())
            .filter(|s| self == &s.function)
            .map(|s| DataSinkRef {
                call_site: self,
                sink: s,
            })
            .collect();
        all.sort_by_key(|s| s.sink.arg_slot);
        all
    }

    pub fn flows_to(&self, sink: &DataSinkRef) -> bool {
        let mut seen = HashSet::new();
        let mut queue: Vec<_> = self
            .ctrl
            .ctrl
            .flow
            .0
            .get(&dfpp::desc::DataSource::FunctionCall(
                self.call_site.clone(),
            ))
            .iter()
            .flat_map(|i| i.iter())
            .collect();
        while let Some(n) = queue.pop() {
            if sink == n {
                return true;
            }
            if !seen.contains(n) {
                seen.insert(n);
                queue.extend(
                    self.ctrl.ctrl
                        .flow
                        .0
                        .get(&dfpp::desc::DataSource::FunctionCall(n.function.clone()))
                        .iter()
                        .flat_map(|s| s.iter()),
                );
            }
        }
        false
    }
}

impl<'g> HasGraph<'g> for &CallSiteRef<'g> {
    fn graph(self) -> &'g ProgramDescription {
        self.function.graph()
    }
}

pub struct DataSinkRef<'g> {
    call_site: &'g CallSiteRef<'g>,
    sink: &'g dfpp::desc::DataSink,
}

impl<'g> HasGraph<'g> for &DataSinkRef<'g> {
    fn graph(self) -> &'g ProgramDescription {
        self.call_site.graph()
    }
}

impl PartialEq<dfpp::desc::DataSink> for DataSinkRef<'_> {
    fn eq(&self, other: &dfpp::desc::DataSink) -> bool {
        self.sink == other
    }
}
