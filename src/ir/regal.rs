use flowistry::{
    extensions::RecurseSelector,
    mir::{borrowck_facts, control_dependencies::ControlDependencies, utils::BodyExt},
};

use super::GLI;
use crate::{
    ana::{
        algebra::{self, Equality, Term},
        df,
    },
    hir::def_id::LocalDefId,
    mir::{self, Field, HasLocalDecls, Location},
    rust::{
        rustc_ast,
        rustc_hir::{def_id::DefId, BodyId},
        rustc_index::bit_set::HybridBitSet,
        rustc_index::vec::IndexVec,
    },
    utils::{
        body_name_pls, dump_file_pls, time, write_sep, AsFnAndArgs, AsFnAndArgsErr,
        DisplayViaDebug, IntoLocalDefId, LocationExt, Print,
    },
    DbgArgs, Either, HashMap, HashSet, TyCtxt,
};

use std::fmt::{Display, Write};

newtype_index!(
    pub struct ArgumentIndex {
        DEBUG_FORMAT = "arg{}"
    }
);

impl Display for ArgumentIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a{}", self.as_usize())
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Hash, Copy, Ord, PartialOrd)]
pub enum TargetPlace {
    Return,
    Argument(ArgumentIndex),
}

#[derive(Hash, Eq, PartialEq, Debug, Copy, Clone)]
pub enum Target<L> {
    Call(L),
    Argument(ArgumentIndex),
}

impl<L> Target<L> {
    pub fn map_location<L0, F: FnMut(&L) -> L0>(&self, mut f: F) -> Target<L0> {
        match self {
            Target::Argument(a) => Target::Argument(*a),
            Target::Call(l) => Target::Call(f(l)),
        }
    }
}

impl<L: Display> Display for Target<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Call(loc) => write!(f, "{loc}"),
            Target::Argument(a) => a.fmt(f),
        }
    }
}

#[derive(Debug)]
pub struct Call<D> {
    pub function: DefId,
    pub arguments: IndexVec<ArgumentIndex, Option<(mir::Local, D)>>,
    pub return_to: mir::Local,
    pub ctrl_deps: D,
}

impl<D> Call<D> {
    pub fn argument_locals(&self) -> impl Iterator<Item = mir::Local> + '_ {
        self.arguments
            .iter()
            .filter_map(|a| a.as_ref().map(|i| i.0))
    }
}

struct NeverInline;

impl RecurseSelector for NeverInline {
    fn is_selected<'tcx>(&self, _tcx: TyCtxt<'tcx>, _tk: &mir::TerminatorKind<'tcx>) -> bool {
        false
    }
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub struct RelativePlace<L> {
    pub location: L,
    pub place: TargetPlace,
}

impl<L: Display> Display for RelativePlace<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} @ {}", self.location, self.place)
    }
}

pub type Dependencies<L> = HashSet<Target<L>>;

fn fmt_deps<L: Display>(
    deps: &Dependencies<L>,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    f.write_char('{')?;
    let mut first_dep = true;
    for dep in deps {
        if first_dep {
            first_dep = false;
        } else {
            f.write_str(", ")?;
        }
        write!(f, "{dep}")?;
    }
    f.write_char('}')
}

impl<L: Display> Display for Call<Dependencies<L>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_char('(')?;
        write_sep(f, ", ", self.arguments.iter(), |elem, f| {
            if let Some((place, deps)) = elem {
                fmt_deps(&deps, f)?;
                write!(f, " with {place:?}")
            } else {
                f.write_str("{}")
            }
        })?;
        write!(f, ") ctrl:")?;
        fmt_deps(&self.ctrl_deps, f)?;
        write!(f, " return:{:?}", self.return_to)?;
        write!(f, " {:?}", self.function)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Copy, Ord, PartialOrd)]
pub enum SimpleLocation<C> {
    Return,
    Argument(ArgumentIndex),
    Call(C),
}

impl<L> SimpleLocation<L> {
    pub fn map_location<L0, F: FnMut(&L) -> L0>(&self, mut f: F) -> SimpleLocation<L0> {
        use SimpleLocation::*;
        match self {
            Argument(a) => Argument(*a),
            Call(l) => Call(f(l)),
            Return => Return,
        }
    }
}

impl<D: std::fmt::Display> std::fmt::Display for SimpleLocation<(D, DefId)> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use SimpleLocation::*;
        match self {
            Return => f.write_str("ret"),
            Argument(a) => write!(f, "{a:?}"),
            Call((gloc, did)) => write!(f, "{gloc} ({did:?})"),
        }
    }
}

impl<D: std::fmt::Display> std::fmt::Display for SimpleLocation<RelativePlace<D>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use SimpleLocation::*;
        match self {
            Return => f.write_str("ret"),
            Argument(a) => write!(f, "{a:?}"),
            Call(c) => write!(f, "{c}"),
        }
    }
}
#[derive(Debug)]
pub struct Body<L> {
    pub calls: HashMap<L, Call<Dependencies<L>>>,
    pub return_deps: Dependencies<L>,
    pub return_arg_deps: Vec<Dependencies<L>>,
    pub equations: Vec<algebra::Equality<DisplayViaDebug<mir::Local>, DisplayViaDebug<Field>>>,
}

impl<L: Display + Ord> Display for Body<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ordered = self.calls.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|t| t.0);
        for (loc, call) in ordered {
            writeln!(f, "{:<6}: {}", format!("{}", loc), call)?
        }
        write!(f, "return: ")?;
        fmt_deps(&self.return_deps, f)?;
        writeln!(f)?;
        write!(f, "return args: (")?;
        let mut first_arg = true;
        for arg in &self.return_arg_deps {
            if first_arg {
                first_arg = false;
            } else {
                f.write_str(", ")?;
            }
            fmt_deps(arg, f)?;
        }
        f.write_char(')')?;
        writeln!(f)?;
        writeln!(f, "equations:")?;
        for eq in self.equations.iter() {
            writeln!(f, "  {eq}")?;
        }
        Ok(())
    }
}

fn get_highest_local(body: &mir::Body) -> mir::Local {
    use mir::visit::Visitor;
    struct Extractor(Option<mir::Local>);
    impl Visitor<'_> for Extractor {
        fn visit_local(
            &mut self,
            local: &mir::Local,
            _context: mir::visit::PlaceContext,
            _location: Location,
        ) {
            let m = self.0.get_or_insert(*local);
            if *m < *local {
                *m = *local;
            }
        }
    }
    let mut e = Extractor(None);
    e.visit_body(body);
    e.0.unwrap()
}

impl Body<DisplayViaDebug<Location>> {
    pub fn construct<'tcx, I: IntoIterator<Item = algebra::MirEquation>>(
        flow_analysis: df::FlowResults<'_, 'tcx, '_>,
        equations: I,
        tcx: TyCtxt<'tcx>,
        def_id: LocalDefId,
        body_with_facts: &'tcx flowistry::mir::borrowck_facts::CachedSimplifedBodyWithFacts<'tcx>,
    ) -> Self {
        let name = body_name_pls(tcx, def_id).name;
        time(&format!("Regal Body Construction of {name}"), || {
            let body = flow_analysis.analysis.body;
            let ctrl_ana = &flow_analysis.analysis.control_dependencies;
            let non_transitive_aliases =
                crate::ana::non_transitive_aliases::compute(tcx, def_id, body_with_facts);

            let dependencies_for = |location: DisplayViaDebug<_>,
                                    arg,
                                    is_mut_arg|
             -> Dependencies<DisplayViaDebug<_>> {
                use rustc_ast::Mutability;
                debug!("Dependencies for {arg:?} at {location}");
                let ana = flow_analysis.state_at(*location);
                let mutability = if false && is_mut_arg {
                    Mutability::Mut
                } else {
                    Mutability::Not
                };
                // Not sure this is necessary anymore because I changed the analysis
                // to transitively propagate in cases where a subplace is modified
                let reachable_values = non_transitive_aliases.reachable_values(arg, mutability);
                debug!("Reachable values for {arg:?} are {reachable_values:?}");
                debug!(
                    "  Children are {:?}",
                    reachable_values
                        .into_iter()
                        .flat_map(|a| non_transitive_aliases.children(*a))
                        .collect::<Vec<_>>()
                );
                let deps = reachable_values
                    .into_iter()
                    .flat_map(|p| non_transitive_aliases.children(*p))
                    // Commenting out this filter because reachable values doesn't
                    // always contain all relevant subplaces
                    //.filter(|p| !is_mut_arg || p != &arg)
                    .flat_map(|place| ana.deps(non_transitive_aliases.normalize(place)))
                    .map(|&(dep_loc, _dep_place)| {
                        let dep_loc = DisplayViaDebug(dep_loc);
                        if dep_loc.is_real(body) {
                            Target::Call(dep_loc)
                        } else {
                            Target::Argument(ArgumentIndex::from_usize(dep_loc.statement_index - 1))
                        }
                    })
                    .collect();
                debug!("  Registering dependencies {deps:?}");
                deps
            };
            let mut call_argument_equations = HashSet::new();
            let mut next_new_local = get_highest_local(body);
            let calls = body
                .basic_blocks()
                .iter_enumerated()
                .filter_map(|(bb, bbdat)| {
                    let (function, simple_args, ret) = match bbdat.terminator().as_fn_and_args() {
                        Ok(p) => p,
                        Err(AsFnAndArgsErr::NotAFunctionCall) => return None,
                        Err(e) => panic!("{e:?}"),
                    };
                    let bbloc = DisplayViaDebug(body.terminator_loc(bb));

                    let arguments = IndexVec::from_raw(
                        simple_args
                            .into_iter()
                            .map(|arg| {
                                arg.map(|a| {
                                    let local = if a.projection.is_empty() {
                                        a.local
                                    } else {
                                        use crate::rust::rustc_index::vec::Idx;
                                        next_new_local.increment_by(1);
                                        call_argument_equations.insert(Equality::new(
                                            Term::new_base(DisplayViaDebug(next_new_local)),
                                            Term::from(a),
                                        ));
                                        next_new_local
                                    };
                                    (local, dependencies_for(bbloc, a, false))
                                })
                            })
                            .collect(),
                    );
                    let ctrl_deps = recursive_ctrl_deps(ctrl_ana, bb, body, dependencies_for);
                    let return_place = ret.unwrap().0;
                    assert!(return_place.projection.is_empty());
                    Some((
                        bbloc,
                        Call {
                            function,
                            arguments,
                            ctrl_deps,
                            return_to: return_place.local,
                        },
                    ))
                })
                .collect();
            let mut return_arg_deps: Vec<(mir::Place<'tcx>, _)> = body
                .args_iter()
                .flat_map(|a| {
                    let place = mir::Place::from(a);
                    let local_decls = body.local_decls();
                    let ty = place.ty(local_decls, tcx).ty;
                    if ty.is_mutable_ptr() {
                        Either::Left(
                            Some(place.project_deeper(&[mir::PlaceElem::Deref], tcx)).into_iter(),
                        )
                    } else if ty.is_generator() {
                        debug!(
                            "{ty:?} is a generator with children {:?}",
                            non_transitive_aliases.children(place)
                        );
                        Either::Right(
                            non_transitive_aliases
                                .children(place)
                                .into_iter()
                                .filter_map(|child| {
                                    child.ty(local_decls, tcx).ty.is_mutable_ptr().then(|| {
                                        child.project_deeper(&[mir::PlaceElem::Deref], tcx)
                                    })
                                }),
                        )
                    } else {
                        Either::Left(None.into_iter())
                    }
                })
                .map(|p| (p, HashSet::new()))
                .collect();
            debug!("Return arguments are {return_arg_deps:?}");
            let return_deps = body
                .all_returns()
                .map(DisplayViaDebug)
                .flat_map(|loc| {
                    return_arg_deps.iter_mut().for_each(|(i, s)| {
                        debug!("Return arg dependencies for {i:?} at {loc}");
                        for d in dependencies_for(loc, *i, true) {
                            debug!("  adding {d}");
                            s.insert(d);
                        }
                    });
                    dependencies_for(loc, mir::Place::return_place(), false)
                        .clone()
                        .into_iter()
                })
                .collect();

            let equations = equations
                .into_iter()
                .chain(call_argument_equations)
                .collect::<Vec<_>>();

            Self {
                calls,
                return_deps,
                return_arg_deps: return_arg_deps.into_iter().map(|(_, s)| s).collect(),
                equations,
            }
        })
    }
}

/// Uhh, so this function is kinda ugly. It tries to make sure we're not missing
/// control flow edges, but at the same time it also tries to preserve
/// non-transitivity among control flow dependencies. What this means is that if
/// you have a case like
///
/// ```
/// let y = baz();
/// if y {
///   let x = foo();
///   if x {
///     bar(...);
///   }
/// }
/// ```
///
/// Then `foo` will be a control dependency of `bar`, but `baz` will not.
/// Instead that is only a transitive dependency because `baz` is a ctrl
/// dependency of `foo`.
///
/// XXX: These semantics are what I believed we wanted, but we haven't discussed
/// if this is the right thing to do.
fn recursive_ctrl_deps<
    'tcx,
    F: FnMut(
        DisplayViaDebug<Location>,
        mir::Place<'tcx>,
        bool,
    ) -> Dependencies<DisplayViaDebug<Location>>,
>(
    ctrl_ana: &ControlDependencies,
    bb: mir::BasicBlock,
    body: &mir::Body<'tcx>,
    mut dependencies_for: F,
) -> Dependencies<DisplayViaDebug<Location>> {
    debug!(
        "Ctrl deps\n{}",
        Print(|f| {
            for (b, _) in body.basic_blocks().iter_enumerated() {
                writeln!(f, "{b:?}: {:?}", ctrl_ana.dependent_on(b))?;
            }
            Ok(())
        })
    );
    let mut seen = ctrl_ana
        .dependent_on(bb)
        .cloned()
        .unwrap_or_else(|| HybridBitSet::new_empty(0));
    debug!(
        "Initial ctrl flow of {bb:?} depends on {:?}",
        seen.iter().collect::<Vec<_>>()
    );
    let mut queue = seen.iter().collect::<Vec<_>>();
    let mut dependencies = Dependencies::new();
    while let Some(block) = queue.pop() {
        seen.insert(block);
        let terminator = body.basic_blocks()[block].terminator();
        if let mir::TerminatorKind::SwitchInt { discr, .. } = &terminator.kind {
            if let Some(discr_place) = discr.place() {
                let deps = dependencies_for(
                    DisplayViaDebug(body.terminator_loc(block)),
                    discr_place,
                    false,
                );
                for d in &deps {
                    if let Target::Call(loc) = d {
                        seen.insert(loc.block);
                    }
                }
                dependencies.extend(deps);

                if let Some(mut switch_deps) = ctrl_ana.dependent_on(block).cloned() {
                    switch_deps.subtract(&seen);
                    queue.extend(switch_deps.iter());
                }

                // This is where things go off the rails.
                //
                // The reason this is so complicated is because rustc desugars
                // `&&` and `||` in an annoying way. The details are explained
                // in
                // https://www.notion.so/justus-adam/Control-flow-with-non-fn-statement-does-not-create-the-ctrl_flow-relation-correctly-3993e8fd86d54f51bfa75fde447b81ec
                let predecessors = &body.predecessors()[block];
                if predecessors.len() > 1 {
                    enum SetResult<A> {
                        Uninit,
                        Unequal,
                        Set(A),
                    }
                    if let SetResult::Set(parent_deps) = {
                        use mir::visit::Visitor;
                        struct AssignsCheck<'tcx> {
                            target: mir::Place<'tcx>,
                            was_assigned: bool,
                        }
                        impl<'tcx> Visitor<'tcx> for AssignsCheck<'tcx> {
                            fn visit_assign(
                                &mut self,
                                place: &mir::Place<'tcx>,
                                _rvalue: &mir::Rvalue<'tcx>,
                                _location: Location,
                            ) {
                                self.was_assigned |= *place == self.target;
                            }
                            fn visit_terminator(
                                &mut self,
                                terminator: &mir::Terminator<'tcx>,
                                _location: Location,
                            ) {
                                match terminator.kind {
                                    mir::TerminatorKind::Call {
                                        destination: Some((dest, _)),
                                        ..
                                    } => self.was_assigned |= dest == self.target,
                                    _ => (),
                                }
                            }
                        }

                        predecessors
                            .iter()
                            .fold(SetResult::Uninit, |prev_deps, &block| {
                                if matches!(prev_deps, SetResult::Unequal) {
                                    debug!("Already unequal");
                                    return SetResult::Unequal;
                                }
                                let ctrl_deps =
                                    if let Some(ctrl_deps) = ctrl_ana.dependent_on(block) {
                                        ctrl_deps
                                    } else {
                                        debug!("No Deps");
                                        return SetResult::Unequal;
                                    };
                                let data = &body.basic_blocks()[block];
                                let mut check = AssignsCheck {
                                    target: discr_place,
                                    was_assigned: false,
                                };
                                check.visit_basic_block_data(block, data);
                                if !check.was_assigned {
                                    debug!("{discr_place:?} not assigned");
                                    return SetResult::Unequal;
                                }
                                match prev_deps {
                                    SetResult::Uninit => SetResult::Set(ctrl_deps),
                                    SetResult::Set(other)
                                        if !other.superset(ctrl_deps)
                                            || !ctrl_deps.superset(other) =>
                                    {
                                        debug!("Unequal");
                                        SetResult::Unequal
                                    }
                                    _ => prev_deps,
                                }
                            })
                    } {
                        debug!("Also exploring parents {parent_deps:?}");
                        queue.extend(parent_deps.iter());
                    }
                }
            }
        }
    }
    dependencies
}

pub fn compute_from_body_id(
    dbg_opts: &DbgArgs,
    body_id: BodyId,
    tcx: TyCtxt,
    gli: GLI,
) -> Body<DisplayViaDebug<Location>> {
    let local_def_id = body_id.into_local_def_id(tcx);
    info!("Analyzing function {}", body_name_pls(tcx, body_id));
    let body_with_facts = borrowck_facts::get_body_with_borrowck_facts(tcx, local_def_id);
    let body = body_with_facts.simplified_body();
    let flow = df::compute_flow_internal(tcx, gli, body_id, body_with_facts);
    if dbg_opts.dump_callee_mir() {
        mir::pretty::write_mir_fn(
            tcx,
            body,
            &mut |_, _| Ok(()),
            &mut dump_file_pls(tcx, body_id, "mir").unwrap(),
        )
        .unwrap();
    }
    if dbg_opts.dump_dataflow_analysis_result() {
        use std::io::Write;
        let ref mut states_out = dump_file_pls(tcx, body_id, "df").unwrap();
        for l in body.all_locations() {
            writeln!(states_out, "{l:?}: {}", flow.state_at(l)).unwrap();
        }
    }
    let equations = algebra::extract_equations(tcx, body);
    let r = Body::construct(flow, equations, tcx, local_def_id, body_with_facts);
    if dbg_opts.dump_regal_ir() {
        let mut out = dump_file_pls(tcx, body_id, "regal").unwrap();
        use std::io::Write;
        write!(&mut out, "{}", r).unwrap();
    }
    r
}
