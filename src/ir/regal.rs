use flowistry::{
    extensions::RecurseSelector,
    mir::{borrowck_facts, utils::BodyExt},
};

use super::GLI;
use crate::{
    ana::{
        algebra::{self, Equality, Term},
        df,
    },
    hir::def_id::LocalDefId,
    mir::{self, Field, Location},
    rust::{
        rustc_ast,
        rustc_hir::{def_id::DefId, BodyId},
        rustc_index::vec::IndexVec,
    },
    utils::{
        body_name_pls, outfile_pls, places_read, time, AsFnAndArgs, AsFnAndArgsErr,
        DisplayViaDebug, LocationExt,
    },
    Either, HashMap, HashSet, TyCtxt,
};

use std::{
    borrow::Cow,
    fmt::{Display, Write},
};

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
    pub arguments: IndexVec<ArgumentIndex, D>,
    pub ctrl_deps: D,
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
        let mut first = true;
        for arg in self.arguments.iter() {
            if first {
                first = false;
            } else {
                f.write_str(", ")?;
            }
            fmt_deps(arg, f)?;
        }
        write!(f, ")   {:?}", self.function)?;
        Ok(())
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
    pub equations: Vec<algebra::Equality<SimpleLocation<RelativePlace<L>>, DisplayViaDebug<Field>>>,
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
            let mut place_table: HashMap<
                mir::Local,
                Vec<SimpleLocation<RelativePlace<DisplayViaDebug<Location>>>>,
            > = body
                .args_iter()
                .enumerate()
                .map(|(idx, l)| {
                    (
                        l,
                        vec![
                            SimpleLocation::Argument(ArgumentIndex::from_usize(idx)),
                            //SimpleLocation::Return(Some(ArgumentIndex::from_usize(idx)))
                        ],
                    )
                })
                .chain([(mir::RETURN_PLACE, vec![SimpleLocation::Return])])
                .collect();
            let dependencies_for = |location: DisplayViaDebug<_>, arg, is_mut_arg| {
                use rustc_ast::Mutability;
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
                    "  Transitive reachable values are {:?}",
                    flow_analysis
                        .analysis
                        .aliases
                        .reachable_values(arg, mutability)
                );
                let deps = reachable_values
                    .into_iter()
                    .flat_map(|p| non_transitive_aliases.children(*p))
                    // Commenting out this filter because reachable values doesn't
                    // always contain all relevant subplaces
                    //.filter(|p| !is_mut_arg || p != &arg)
                    .flat_map(|place| ana.deps(place))
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
                    let (function, simple_args, _) = match bbdat.terminator().as_fn_and_args() {
                        Ok(p) => p,
                        Err(AsFnAndArgsErr::NotAFunctionCall) => return None,
                        Err(e) => panic!("{e:?}"),
                    };
                    let bbloc = DisplayViaDebug(body.terminator_loc(bb));

                    let mk_rp = |place| {
                        SimpleLocation::Call(RelativePlace {
                            location: bbloc,
                            place,
                        })
                    };

                    let (operands, target_ret) =
                        if let mir::TerminatorKind::Call {
                            args, destination, ..
                        } = &body.stmt_at(*bbloc).right().unwrap().kind
                        {
                            (args, destination)
                        } else {
                            unreachable!()
                        };

                    for (idx, place) in flowistry::mir::utils::arg_places(operands.as_slice()) {
                        use crate::rust::rustc_index::vec::Idx;
                        let local = if place.projection.is_empty() {
                            place.local
                        } else {
                            next_new_local.increment_by(1);
                            call_argument_equations.insert(Equality::new(
                                Term::new_base(DisplayViaDebug(next_new_local)),
                                Term::from(place),
                            ));
                            next_new_local
                        };
                        place_table
                            .entry(local)
                            .or_insert_with(Vec::new)
                            .push(mk_rp(TargetPlace::Argument(ArgumentIndex::from_usize(idx))));
                    }
                    let target_ret = target_ret.unwrap().0;
                    assert!(target_ret.projection.is_empty());
                    place_table
                        .entry(target_ret.local)
                        .or_insert_with(Vec::new)
                        .push(mk_rp(TargetPlace::Return));

                    let arguments = IndexVec::from_raw(
                        simple_args
                            .into_iter()
                            .map(|arg| {
                                arg.map_or_else(Dependencies::default, |a| {
                                    dependencies_for(bbloc, a, false)
                                })
                            })
                            .collect(),
                    );
                    let ctrl_deps = ctrl_ana
                        .dependent_on(bb)
                        .into_iter()
                        .flat_map(|s| s.iter())
                        .flat_map(|block| {
                            let terminator = body.basic_blocks()[block].terminator();
                            if let mir::TerminatorKind::SwitchInt { discr, .. } = &terminator.kind {
                                discr.place().map(|discr_place| {
                                    dependencies_for(
                                        DisplayViaDebug(body.terminator_loc(block)),
                                        discr_place,
                                        false,
                                    )
                                })
                            } else {
                                None
                            }
                            .into_iter()
                            .flatten()
                        })
                        .collect();
                    Some((
                        bbloc,
                        Call {
                            function,
                            arguments,
                            ctrl_deps,
                        },
                    ))
                })
                .collect();
            let mut return_arg_deps: Vec<(mir::Place<'tcx>, _)> = body
                .args_iter()
                .map(|a| (a.into(), HashSet::new()))
                .collect();
            debug!("Return arguments are {return_arg_deps:?}");
            let return_deps = body
                .all_returns()
                .map(DisplayViaDebug)
                .flat_map(|loc| {
                    return_arg_deps.iter_mut().for_each(|(i, s)| {
                        debug!("Return arg dependencies for {i:?} at {loc}");
                        for d in dependencies_for(loc, *i, true) {
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

            debug!(
                "Equations before simplify:\n{}",
                crate::utils::Print(|f: &mut std::fmt::Formatter<'_>| {
                    for eq in equations.iter() {
                        writeln!(f, "  {eq}")?;
                    }
                    Ok(())
                })
            );
            debug!(
                "And place table\n{}",
                crate::utils::Print(|f: &mut std::fmt::Formatter<'_>| {
                    for (k, v) in place_table.iter() {
                        write!(f, "  {k:?}: ")?;
                        let mut first = true;
                        for t in v {
                            if first {
                                first = false;
                            } else {
                                f.write_str(", ")?;
                            }
                            t.fmt(f)?;
                        }
                        writeln!(f)?;
                    }
                    Ok(())
                })
            );
            let equations = time(&format!("Equation Simplification of {name}"), || {
                algebra::rebase_simplify(
                    equations.into_iter().map(Cow::Owned).chain(
                        place_table.keys().map(|k| DisplayViaDebug(*k)).map(|k| {
                            Cow::Owned(Equality::new(Term::new_base(k), Term::new_base(k)))
                        }),
                    ),
                    |base| {
                        place_table
                            .get(base)
                            .cloned()
                            .map(Either::Left)
                            .unwrap_or(Either::Right(*base))
                    },
                )
            });
            debug!(
                "Equations after simplify:\n{}",
                crate::utils::Print(|f: &mut std::fmt::Formatter<'_>| {
                    for eq in equations.iter() {
                        writeln!(f, "  {eq}")?;
                    }
                    Ok(())
                })
            );
            Self {
                calls,
                return_deps,
                return_arg_deps: return_arg_deps.into_iter().map(|(_, s)| s).collect(),
                equations,
            }
        })
    }
}

pub fn compute_from_body_id(
    body_id: BodyId,
    tcx: TyCtxt,
    gli: GLI,
) -> Body<DisplayViaDebug<Location>> {
    let local_def_id = tcx.hir().body_owner_def_id(body_id);
    let target_name = body_name_pls(tcx, body_id);
    info!("Analyzing function {target_name}");
    let body_with_facts = borrowck_facts::get_body_with_borrowck_facts(tcx, local_def_id);
    let body = body_with_facts.simplified_body();
    let flow = df::compute_flow_internal(tcx, gli, body_id, body_with_facts);
    mir::pretty::write_mir_fn(
        tcx,
        body,
        &mut |_, _| Ok(()),
        &mut outfile_pls(&format!("{}.mir", target_name)).unwrap(),
    )
    .unwrap();
    let ref mut states_out = outfile_pls(&format!("{}.df", target_name)).unwrap();
    for l in body.all_locations() {
        writeln!(states_out, "{l:?}: {}", flow.state_at(l)).unwrap();
    }
    let equations = algebra::extract_equations(tcx, body);
    let r = Body::construct(flow, equations, tcx, local_def_id, body_with_facts);
    let mut out = outfile_pls(&format!("{}.regal", target_name)).unwrap();
    use std::io::Write;
    write!(&mut out, "{}", r).unwrap();
    r
}
