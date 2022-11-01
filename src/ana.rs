use std::{borrow::Cow, cell::RefCell, rc::Rc};

use crate::{
    dbg::{dump_non_transitive_graph_and_body, self}, desc::*, rust::*, sah::HashVerifications, Either,
    HashMap, HashSet,
};

use hir::{
    def_id::DefId,
    hir_id::HirId,
    intravisit::{self, FnKind},
    BodyId,
};
use rustc_middle::{
    hir::nested_filter::OnlyBodies,
    ty::{self, TyCtxt},
};
use rustc_span::{symbol::Ident, Span, Symbol};

use flowistry::{
    indexed::{impls::LocationDomain, IndexedDomain},
    infoflow::{self, FlowDomain},
    mir::{borrowck_facts, utils::BodyExt},
};

pub type AttrMatchT = Vec<Symbol>;

trait MetaItemMatch {
    fn match_extract<A, F: Fn(&ast::MacArgs) -> A>(&self, p: &[Symbol], f: F) -> Option<A>;
    fn matches_path(&self, p: &[Symbol]) -> bool {
        self.match_extract(p, |_| ()).is_some()
    }
}

impl MetaItemMatch for ast::Attribute {
    fn match_extract<A, F: Fn(&ast::MacArgs) -> A>(&self, p: &[Symbol], f: F) -> Option<A> {
        match &self.kind {
            rustc_ast::ast::AttrKind::Normal(ast::AttrItem { path, args, .. }, _)
                if path.segments.len() == p.len()
                    && path
                        .segments
                        .iter()
                        .zip(p)
                        .all(|(seg, i)| seg.ident.name == *i) =>
            {
                Some(f(args))
            }
            _ => None,
        }
    }
}

fn ty_def(ty: ty::Ty) -> Option<DefId> {
    match ty.kind() {
        ty::TyKind::Adt(def, _) => Some(def.did()),
        ty::TyKind::Foreign(did)
        | ty::TyKind::FnDef(did, _)
        | ty::TyKind::Closure(did, _)
        | ty::TyKind::Generator(did, _, _)
        | ty::TyKind::Opaque(did, _) => Some(*did),
        _ => None,
    }
}

fn generic_arg_as_type(a: ty::subst::GenericArg) -> Option<ty::Ty> {
    match a.unpack() {
        ty::subst::GenericArgKind::Type(t) => Some(t),
        _ => None,
    }
}

trait TerminatorExt<'tcx> {
    fn as_fn_and_args(
        &self,
    ) -> Result<(DefId, Vec<Option<mir::Place<'tcx>>>, mir::Place<'tcx>), &'static str>;
}

impl<'tcx> TerminatorExt<'tcx> for mir::Terminator<'tcx> {
    fn as_fn_and_args(
        &self,
    ) -> Result<(DefId, Vec<Option<mir::Place<'tcx>>>, mir::Place<'tcx>), &'static str> {
        match &self.kind {
            mir::TerminatorKind::Call {
                func,
                args,
                destination,
                ..
            } => {
                let defid = match func.constant().ok_or("Not a constant")? {
                    mir::Constant {
                        literal: mir::ConstantKind::Val(_, ty),
                        ..
                    } => match ty.kind() {
                        ty::FnDef(defid, _) | ty::Closure(defid, _) => Ok(*defid),
                        _ => Err("Not function type"),
                    },
                    _ => Err("Not value level constant"),
                }?;
                Ok((
                    defid,
                    args.iter()
                        .map(|a| match a {
                            mir::Operand::Move(p) | mir::Operand::Copy(p) => Some(*p),
                            mir::Operand::Constant(_) => None,
                        })
                        .collect(),
                    destination.ok_or("missing return place")?.0,
                ))
            }
            _ => Err("Not a function call".into()),
        }
    }
}
/// A struct that can be used to apply a `FnMut` to every `Place` in a MIR
/// object via the visit::Visitor` trait. Usually used to accumulate information
/// about the places.
pub struct PlaceVisitor<F>(pub F);

impl<'tcx, F: FnMut(&mir::Place<'tcx>)> mir::visit::Visitor<'tcx> for PlaceVisitor<F> {
    fn visit_place(
        &mut self,
        place: &mir::Place<'tcx>,
        _context: mir::visit::PlaceContext,
        _location: mir::Location,
    ) {
        self.0(place)
    }
}

/// This function deals with the fact that flowistry uses special locations to
/// refer to function arguments. Those locations are not recognized the rustc
/// functions that operate on MIR and thus need to be filtered before doing
/// things such as indexing into a `mir::Body`.
pub fn is_real_location(body: &mir::Body, l: mir::Location) -> bool {
    body.basic_blocks().get(l.block).map(|bb| 
            // Its `<=` because if len == statement_index it refers to the
            // terminator
            // https://doc.rust-lang.org/nightly/nightly-rustc/rustc_middle/mir/struct.Location.html
            l.statement_index <= bb.statements.len())
        == Some(true)
}

pub struct Visitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    opts: &'static crate::Args,
    marked_objects: HashMap<HirId, (Vec<Annotation>, ObjectType)>,
    marked_stmts: HashMap<HirId, ((Vec<Annotation>, usize), Span, DefId)>,
    functions_to_analyze: Vec<(Ident, BodyId, &'tcx rustc_hir::FnDecl<'tcx>)>,
}

type CallSiteAnnotations = HashMap<DefId, (Vec<Annotation>, usize)>;

pub struct Flow<'a, 'tcx> {
    pub kind: FlowKind<'a, 'tcx>,
    pub domain: Rc<LocationDomain>,
}

pub enum FlowKind<'a, 'tcx> {
    Transitive(
        flowistry::infoflow::FlowResults<'a, 'tcx, flowistry::infoflow::TransitiveFlowDomain<'tcx>>,
    ),
    NonTransitive(
        flowistry::infoflow::FlowResults<
            'a,
            'tcx,
            flowistry::infoflow::NonTransitiveFlowDomain<'tcx>,
        >,
    ),
    NonTransitiveShrunk {
        original_flow: flowistry::infoflow::FlowResults<
            'a,
            'tcx,
            flowistry::infoflow::NonTransitiveFlowDomain<'tcx>,
        >,
        shrunk: NonTransitiveGraph<'tcx>,
    },
}

impl<'a, 'tcx> Flow<'a, 'tcx> {
    #[allow(dead_code)]
    fn is_transitive(&self) -> bool {
        matches!(self.kind, FlowKind::Transitive(_))
    }

    fn as_some_non_transitive_graph(
        &self,
    ) -> Option<crate::dbg::SomeNoneTransitiveGraph<'tcx, 'a, '_>> {
        match &self.kind {
            FlowKind::Transitive(_) => None,
            FlowKind::NonTransitive(t) => Some(Either::Right(&t)),
            FlowKind::NonTransitiveShrunk { shrunk, .. } => Some(Either::Left(&shrunk)),
        }
    }

    #[allow(dead_code)]
    fn aliases(&self) -> &flowistry::mir::aliases::Aliases<'a, 'tcx> {
        match &self.kind {
            FlowKind::NonTransitive(a) => &a.analysis.aliases,
            FlowKind::Transitive(a) => &a.analysis.aliases,
            FlowKind::NonTransitiveShrunk { original_flow, .. } => &original_flow.analysis.aliases,
        }
    }

    fn compute(
        opts: &crate::AnalysisCtrl,
        tcx: TyCtxt<'tcx>,
        body_id: BodyId,
        body_with_facts: &'a crate::rust::rustc_borrowck::BodyWithBorrowckFacts<'tcx>,
    ) -> Self {
        let body = &body_with_facts.body;
        let domain = LocationDomain::new(body);
        if opts.use_transitive_graph {
            Self {
                kind: FlowKind::Transitive(infoflow::compute_flow(tcx, body_id, body_with_facts)),
                domain,
            }
        } else {
            let original_flow = infoflow::compute_flow_nontransitive(tcx, body_id, body_with_facts);
            if opts.no_shrink_flow_domains {
                Self {
                    kind: FlowKind::NonTransitive(original_flow),
                    domain,
                }
            } else {
                let mut locations = body
                    .all_locations()
                    .into_iter()
                    .filter(|l| body.stmt_at(*l).is_right())
                    .collect::<Vec<_>>();
                locations.extend(flowistry::indexed::impls::arg_locations(body).1);
                let num_real_locations = locations.len();
                let shrunk_domain = Rc::new(LocationDomain::from_raw(
                    flowistry::indexed::DefaultDomain::new(locations),
                    domain.arg_block(),
                    num_real_locations,
                ));
                let shrunk = shrink_flow_domain(&original_flow, &shrunk_domain, body, tcx);
                Self {
                    kind: FlowKind::NonTransitiveShrunk {
                        original_flow,
                        shrunk,
                    },
                    domain: shrunk_domain,
                }
            }
        }
    }

    pub fn get_row<'b>(
        &'b self,
        l: mir::Location,
    ) -> &'b IndexMatrix<mir::Place<'tcx>, mir::Location> {
        match &self.kind {
            FlowKind::NonTransitive(hm) => hm.state_at(l).matrix(),
            FlowKind::Transitive(fa) => fa.state_at(l),
            FlowKind::NonTransitiveShrunk { shrunk, .. } => shrunk.get(&l).unwrap(),
        }
    }
}

pub fn mentioned_places_with_provenance<'tcx>(
    l: mir::Location,
    body: &mir::Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> impl Iterator<Item = mir::Place<'tcx>> {
    use flowistry::mir::utils::PlaceExt;
    extract_places(l, body, false)
        .into_iter()
        .flat_map(move |place| {
            std::iter::once(place)
                .chain(
                    place
                        .refs_in_projection()
                        .into_iter()
                        .map(|t| mir::Place::from_ref(t.0, tcx)),
                )
                .collect::<Vec<_>>()
                .into_iter()
        })
}

/// The idea of this function is that you can give it Flowistry's analysis and a
/// set of locations, basically a selection of "what you care about" and this
/// function will take care of collapsing all the matrices down so that
/// connections between locations that you care about are preserved, even if
/// transitive hops via locations you **don't care about** are dropped.
///
/// Example if the original MIR had
///
/// ```plain
/// Vec::push(_1, _2)
/// _3 = &_1
/// my_read(_3)
/// ```
///
/// And you instructed this function to only preserve function calls, then the
/// reduced graph would be guaranteed to still have an edge Vec::push -> my_read
fn shrink_flow_domain<'a, 'tcx, D: flowistry::infoflow::FlowDomain<'tcx>>(
    flow: &flowistry::infoflow::FlowResults<'a, 'tcx, D>,
    domain: &Rc<LocationDomain>,
    body: &mir::Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> NonTransitiveGraph<'tcx> {
    let some_result = flow.state_at(mir::Location::START);
    let old_domain = &some_result.matrix().col_domain;
    domain
        .as_vec()
        .iter()
        .filter(|l| is_real_location(body, **l))
        .map(|l| {
            let old_matrix = flow.state_at(*l);
            let mut new_matrix = IndexMatrix::new(&domain);
            old_matrix.matrix().rows().for_each(|(p, s)| {
                let mut queue = s.iter().collect::<Vec<_>>();
                let mut seen = IndexSet::new(old_domain);
                while let Some(g) = queue.pop() {
                    if seen.contains(g) {
                        continue;
                    }
                    seen.insert(g);
                    if domain.contains(g) {
                        new_matrix.insert(p, *g);
                    } else if is_real_location(body, *g) {
                        let state_for_g = flow.state_at(*g).matrix();
                        queue.extend(
                            mentioned_places_with_provenance(*g, body, tcx)
                                .flat_map(|p| state_for_g.row(p)),
                        );
                    }
                }
            });
            (*l, new_matrix)
        })
        .collect()
}

type ReturnModifications<'tcx> = HashMap<Option<mir::Place<'tcx>>, Vec<DataSource>>;
enum ArgumentResolver<'tcx, 'a> {
    Root,
    Nested {
        inner: &'a ArgumentResolver<'tcx, 'a>,
        args: &'a [Option<mir::Place<'tcx>>],
        matrix: &'a IndexMatrix<mir::Place<'tcx>, mir::Location>,
        id: &'a Ident,
        body: &'a mir::Body<'tcx>,
        loc_dom: &'a LocationDomain,
        tcx: TyCtxt<'tcx>,
        accrued_returns: ReturnModifications<'tcx>,
    },
}

impl<'tcx, 'a> ArgumentResolver<'tcx, 'a> {
    fn nested(
        inner: &'a ArgumentResolver<'tcx, 'a>,
        args: &'a [Option<mir::Place<'tcx>>],
        matrix: &'a IndexMatrix<mir::Place<'tcx>, mir::Location>,
        id: &'a Ident,
        body: &'a mir::Body<'tcx>,
        loc_dom: &'a LocationDomain,
        tcx: TyCtxt<'tcx>,
    ) -> Self {
        Self::Nested {
            inner,
            args,
            matrix,
            id,
            body,
            loc_dom,
            tcx,
            accrued_returns: HashMap::new(),
        }
    }
    fn into_returns(self) -> ReturnModifications<'tcx> {
        match self {
            ArgumentResolver::Nested {
                accrued_returns, ..
            } => accrued_returns,
            _ => HashMap::new(),
        }
    }
    fn get_arg_place(&self, i: usize) -> Option<Option<mir::Place<'tcx>>> {
        match self {
            ArgumentResolver::Nested { args, .. } => Some(
                args.get(i)
                    .unwrap_or_else(|| panic!("Index {i} not found in {args:?}"))
                    .clone(),
            ),
            _ => None,
        }
    }
    fn resolve(&'a self, i: usize) -> impl Iterator<Item = DataSource> + 'a {
        match self {
            ArgumentResolver::Root => Box::new(std::iter::once(DataSource::Argument(i)))
                as Box<dyn Iterator<Item = DataSource>>,
            ArgumentResolver::Nested {
                matrix,
                inner,
                id,
                body,
                loc_dom,
                tcx,
                ..
            } => Box::new(
                self.get_arg_place(i - 1 /* I think there's an off-by-one error in how flowistry calculates these argument locations */)
                    .and_then(|a| a)
                    .into_iter()
                    .flat_map(|p| {
                        matrix
                            .row(p)
                            .filter_map(|l| {
                                DataSource::try_from_body(id.name, body, *l, loc_dom, *tcx, inner)
                                    .ok()
                            })
                            .flat_map(|v| v.into_iter())
                    }),
            ) as Box<_>,
        }
    }
    fn register_return(
        &mut self,
        from: DataSource,
        to: Option<mir::Place<'tcx>>,
        flows: &mut Ctrl,
    ) {
        match self {
            ArgumentResolver::Root => flows.add(Cow::Owned(from), DataSink::Return),
            ArgumentResolver::Nested {
                accrued_returns, ..
            } => accrued_returns
                .entry(to)
                .or_insert_with(Vec::new)
                .push(from),
        }
    }
}

impl DataSource {
    fn try_from_body<'tcx>(
        ident: Symbol,
        body: &mir::Body<'tcx>,
        l: mir::Location,
        domain: &LocationDomain,
        tcx: TyCtxt<'tcx>,
        mk_arg: &ArgumentResolver<'tcx, '_>,
    ) -> Result<Vec<Self>, &'static str> {
        let r = if let Some(arg) = domain.location_to_local(l) {
            let v: Vec<_> = mk_arg.resolve(arg.as_usize()).collect();
            debug!(
                "Determined the source is an argument, found {} dependencies",
                v.len()
            );
            v
        } else {
            vec![DataSource::FunctionCall(CallSite {
                called_from: Identifier::new(ident),
                function: identifier_for_fn(
                    tcx,
                    body.stmt_at(l)
                        .right()
                        .ok_or("Not a terminator")?
                        .as_fn_and_args()?
                        .0,
                ),
                location: l,
            })]
        };
        Ok(r)
    }
}

fn node_as_fn<'hir>(
    node: &hir::Node<'hir>,
) -> Option<(&'hir Ident, &'hir hir::def_id::LocalDefId, &'hir BodyId)> {
    if let hir::Node::Item(hir::Item {
        ident,
        def_id,
        kind: hir::ItemKind::Fn(_, _, body_id),
        ..
    }) = node
    {
        Some((ident, def_id, body_id))
    } else {
        None
    }
}

impl<'tcx> Visitor<'tcx> {
    pub(crate) fn new(tcx: TyCtxt<'tcx>, opts: &'static crate::Args) -> Self {
        Self {
            tcx,
            opts,
            marked_objects: HashMap::new(),
            marked_stmts: HashMap::new(),
            functions_to_analyze: vec![],
        }
    }

    fn should_analyze_function(&self, ident: HirId) -> bool {
        self.tcx
            .hir()
            .attrs(ident)
            .iter()
            .any(|a| a.matches_path(&crate::ANALYZE_MARKER))
    }

    pub fn run(mut self) -> std::io::Result<ProgramDescription> {
        let tcx = self.tcx;
        tcx.hir().deep_visit_all_item_likes(&mut self);
        //println!("{:?}\n{:?}\n{:?}", self.marked_sinks, self.marked_sources, self.functions_to_analyze);
        self.analyze()
    }

    fn annotated_subtypes(&self, ty: ty::Ty) -> HashSet<TypeDescriptor> {
        ty.walk()
            .filter_map(|ty| {
                generic_arg_as_type(ty)
                    .and_then(ty_def)
                    .and_then(DefId::as_local)
                    .and_then(|def| {
                        let hid = self.tcx.hir().local_def_id_to_hir_id(def);
                        if self.marked_objects.contains_key(&hid) {
                            Some(Identifier::new(
                                self.tcx
                                    .item_name(self.tcx.hir().local_def_id(hid).to_def_id()),
                            ))
                        } else {
                            None
                        }
                    })
            })
            .collect()
    }

    fn handle_function(
        &self,
        hash_verifications: &mut HashVerifications,
        call_site_annotations: &mut CallSiteAnnotations,
        interesting_fn_defs: &HashMap<DefId, (Vec<Annotation>, usize)>,
        flows: &mut Ctrl,
        seen: &mut HashSet<hir::def_id::LocalDefId>,
        id: Ident,
        b: BodyId,
        local_def_id: hir::def_id::LocalDefId,
        arg_resolver: &mut ArgumentResolver<'tcx, '_>,
    ) {
        let arg_resolver = RefCell::new(arg_resolver);
        fn register_call_site<'tcx>(
            tcx: TyCtxt<'tcx>,
            map: &mut CallSiteAnnotations,
            did: DefId,
            ann: Option<&[Annotation]>,
        ) {
            // This is a bit ugly. This basically just cleans up the fact that
            // when we integrate an annotation on a call, its slightly
            // cumbersome to find out how many arguments the call has. So I just
            // fill in the largest annotated value and then have it merge here
            // later in case we know of more arguments.
            map.entry(did)
                .and_modify(|e| {
                    e.0.extend(ann.iter().flat_map(|a| a.iter()).cloned());
                })
                .or_insert_with(|| {
                    (
                        ann.iter().flat_map(|a| a.iter()).cloned().collect(),
                        tcx.fn_sig(did).skip_binder().inputs().len(),
                    )
                });
        }
        let tcx = self.tcx;
        let body_with_facts = borrowck_facts::get_body_with_borrowck_facts(tcx, local_def_id);

        if self.opts.dbg.dump_ctrl_mir {
            mir::graphviz::write_mir_fn_graphviz(tcx, &body_with_facts.body, false, &mut std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(format!("{}.mir.gv", id.name)).unwrap()).unwrap()
        }

        debug!("{}", id.name);
        let flow = Flow::compute(&self.opts.anactrl, tcx, b, body_with_facts);
        let transitive_flow = infoflow::compute_flow(tcx, b, body_with_facts);

        let body = &body_with_facts.body;
        {
            let resolver_borrow = arg_resolver.borrow();
            let types = body.args_iter().flat_map(|l| {
                let ty = body.local_decls[l].ty;
                let subtypes = self.annotated_subtypes(ty);
                resolver_borrow
                    .resolve(l.as_usize())
                    .map(move |a| (a, subtypes.clone()))
            });
            flows.add_types(types);
        }
        let loc_dom = &flow.domain;
        match flow.as_some_non_transitive_graph() {
            Some(non_t_g) =>
                if self.opts.dbg.dump_non_transitive_graph {
                    crate::dbg::non_transitive_graph_as_dot(
                        &mut std::fs::OpenOptions::new()
                            .truncate(true)
                            .create(true)
                            .write(true)
                            .open(format!("{}.ntg.gv", id.name.as_str()))
                            .unwrap(),
                        body,
                        &non_t_g,
                        &flow.domain,
                        tcx,
                    )
                    .unwrap();
                    info!("Non transitive graph for {} dumped", id.name.as_str());
                } else if self.opts.dbg.dump_serialized_non_transitive_graph {
                    dump_non_transitive_graph_and_body(id, body, &non_t_g, tcx);
                }
            _ if self.opts.dbg.dump_non_transitive_graph || self.opts.dbg.dump_serialized_non_transitive_graph =>
                error!("Told to dump non-transitive graph, but analysis not instructed to make non-transitive graph!"),
            _ => ()
        }
        let mut returns_from_recursed = HashMap::new();
        for (bb, t) in body
            .basic_blocks()
            .iter_enumerated()
            .map(|(bb, bbdat)| (bb, bbdat.terminator()))
        {
            let loc = body.terminator_loc(bb);
            let matrix = flow.get_row(loc);

            if self.opts.dbg.dump_flowistry_matrix {
                info!("Flowistry matrix for {:?}", loc);
                crate::dbg::print_flowistry_matrix(&mut std::io::stdout(), matrix).unwrap();
            }

            let abstraction_info = if let Some((p, args, dest)) = t.as_fn_and_args().ok() {
                let anns = interesting_fn_defs.get(&p).map(|a| a.0.as_slice());
                debug!(
                    "{:?} {} annotations",
                    t.kind,
                    if anns.is_none() {
                        "doesn't have"
                    } else {
                        "has"
                    }
                );
                let stmt_anns = self.statement_anns_by_loc(p, t);
                let bound_sig = tcx.fn_sig(p);
                let interesting_output_types: HashSet<_> =
                    self.annotated_subtypes(bound_sig.skip_binder().output());

                let mentioned_places = args.iter().filter_map(|a| *a).collect::<HashSet<_>>();

                let src_desc = DataSource::FunctionCall(CallSite {
                    function: identifier_for_fn(tcx, p),
                    called_from: Identifier::new(id.name),
                    location: loc,
                });
                if !interesting_output_types.is_empty() {
                    flows.types.0.insert(src_desc, interesting_output_types);
                }

                if let Some(anns) = stmt_anns {
                    for ann in anns.iter().filter_map(Annotation::as_exception_annotation) {
                        hash_verifications.handle(ann, tcx, t, body, loc, matrix);
                    }
                    // TODO this is attaching to functions instead of call
                    // sites. Once we start actually tracking call sites
                    // this needs to be adjusted
                    register_call_site(tcx, call_site_annotations, p, Some(anns));
                }

                if let Some((callee_ident, callee_def_id, callee_body_id)) =
                    tcx.hir().get_if_local(p).and_then(|node| {
                        let nodeinfo = node_as_fn(&node).unwrap_or_else(|| {
                            panic!("Expected local function node, got {node:?}")
                        });
                        if seen.contains(nodeinfo.1) || anns.is_some() {
                            None
                        } else {
                            seen.insert(*nodeinfo.1);
                            Some(nodeinfo)
                        }
                    })
                {
                    debug!("Recursing into callee");
                    let resolver_borrow = arg_resolver.borrow();
                    let mut subresolver = ArgumentResolver::nested(
                        *resolver_borrow,
                        &args,
                        &matrix,
                        &id,
                        &body,
                        &loc_dom,
                        tcx,
                    );
                    self.handle_function(
                        hash_verifications,
                        call_site_annotations,
                        interesting_fn_defs,
                        flows,
                        seen,
                        *callee_ident,
                        *callee_body_id,
                        *callee_def_id,
                        &mut subresolver,
                    );
                    let returns = subresolver
                        .into_returns();
                    debug!("return modification map {returns:?}");
                    returns
                        .into_iter()
                        .for_each(|(p, mods)| {
                            let the_place = p.unwrap_or(dest);
                            for reachable in flow.aliases().reachable_values(the_place, mir::Mutability::Mut).iter().chain(std::iter::once(&the_place)) {
                                if let Some(old) = returns_from_recursed.insert((*reachable, loc), mods.clone()) {
                                    warn!("Duplicate function mutability override for {the_place:?} \n\twith new value \t{mods:?} \n\tand prior value\t{old:?}");
                                }
                            }
                        });
                    None
                } else {
                    debug!("Abstracting callee");
                    register_call_site(tcx, call_site_annotations, p, anns);
                    Some(
                        mentioned_places
                            .into_iter()
                            .map(|r| {
                                (
                                    r,
                                    Either::Right(DataSink::Argument {
                                        function: CallSite {
                                            function: identifier_for_fn(tcx, p),
                                            called_from: Identifier::new(id.name),
                                            location: loc,
                                        },
                                        arg_slot: args
                                            .iter()
                                            .enumerate()
                                            .find(|(_, e)| **e == Some(r))
                                            .unwrap()
                                            .0,
                                    }),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            } else if matches!(t.kind, mir::TerminatorKind::Return) {
                match &flow.kind {
                    FlowKind::NonTransitiveShrunk { original_flow, shrunk } =>
                        debug!("Handling return for {}\n\nPre shrink matrix\n{}\n\nPost shrink matrix\n{}\n\nTransitive matrix\n{}\n", id.name, dbg::PrintableMatrix(original_flow.state_at(loc).matrix()), dbg::PrintableMatrix(&shrunk[&loc]), dbg::PrintableMatrix(transitive_flow.state_at(loc))),
                    _ => (),
                };
                
                Some(
                    std::iter::once((mir::Place::return_place(), Either::Left(None)))
                        .chain(
                            body.args_iter()
                                .enumerate()
                                .filter(|(_, a)| body.local_decls[*a].ty.is_mutable_ptr())
                                .filter_map(|(i, local)| {
                                    debug!("Found mutable argument {:?} at index {i} with arg place {:?}", local, arg_resolver.borrow().get_arg_place(i));
                                    arg_resolver
                                        .borrow()
                                        .get_arg_place(i)
                                        .and_then(|a| a)
                                        .map(|a| (local.into(), Either::Left(Some(a))))
                                }),
                        )
                        .collect(),
                )
            } else {
                None
            };
            if let Some(mentioned_places) = abstraction_info {
                let mut i = 0;
                for (r, sink) in mentioned_places {
                    let deps = matrix.row(r);
                    if sink.is_left() {
                        debug!("Found dependencies {:?}", matrix.row_set(r));
                    }
                    for from in deps
                        .filter_map(|l| {
                            let from_recursed = {
                                let mut all_results = flow.aliases().aliases(r).into_iter().chain(std::iter::once(&r))
                                .filter_map(|p| returns_from_recursed.get(&(*p, *l))).collect::<Vec<_>>();
                                all_results.iter().reduce(|v1, v2| {
                                    assert!(v1 == v2);
                                    v2
                                });
                                all_results.pop()
                            };

                            // Check that if we expect this function to have been recursed into that that actually happened
                            if is_real_location(body, *l) {
                                body.stmt_at(*l).right().map(|t| {
                                    t.as_fn_and_args().ok().map(|fninfo| {
                                        let is_local_function = tcx.hir().get_if_local(fninfo.0).and_then(|n| node_as_fn(&n)).is_some();
                                        let has_annotations = !interesting_fn_defs.get(&fninfo.0).map_or(true, |anns| anns.0.is_empty());

                                        if !(from_recursed.is_some() || !is_local_function || has_annotations) { 
                                            error!("Expected a handled subfunction '{:?}' in '{}', but was not handled yet. Info:\n\thas_recursed:{}\n\tis_local:{is_local_function}\n\thas_annotations:{has_annotations}\n\tsearched_place:{:?}\n\taliases:{:?}\n\treachable_places:{:?}\n\treturns_map{:?}", t.kind, id.name, from_recursed.is_some(), r, flow.aliases().aliases(r), flow.aliases().reachable_values(r, mir::Mutability::Not), returns_from_recursed);
                                        }
                                    })
                                });
                            }

                            from_recursed.map(Cow::Borrowed).or(
                                DataSource::try_from_body(
                                    id.name,
                                    body,
                                    *l,
                                    loc_dom,
                                    tcx,
                                    &arg_resolver.borrow(),
                                )
                                .ok()
                                .map(Cow::Owned),
                            )
                        })
                        .flat_map(|v| v.into_owned().into_iter())
                    {
                        i += 1;
                        match sink.clone() {
                            Either::Right(sink) => flows.add(Cow::Owned(from), sink),
                            Either::Left(to) => {
                                arg_resolver.borrow_mut().register_return(from, to, flows)
                            }
                        }
                    }
                }
                debug!("Found {i} flows into target.");
            }
        }
    }

    /// Handles a single target function
    fn handle_target(
        &self,
        hash_verifications: &mut HashVerifications,
        call_site_annotations: &mut CallSiteAnnotations,
        interesting_fn_defs: &HashMap<DefId, (Vec<Annotation>, usize)>,
        id: Ident,
        b: BodyId,
    ) -> std::io::Result<(Endpoint, Ctrl)> {
        let mut flows = Ctrl::new();
        let local_def_id = self.tcx.hir().body_owner_def_id(b);
        let mut seen = HashSet::new();
        self.handle_function(
            hash_verifications,
            call_site_annotations,
            interesting_fn_defs,
            &mut flows,
            &mut seen,
            id,
            b,
            local_def_id,
            &mut ArgumentResolver::Root,
        );
        Ok((Identifier::new(id.name), flows))
    }

    /// Main analysis driver
    fn analyze(mut self) -> std::io::Result<ProgramDescription> {
        let tcx = self.tcx;
        let mut targets = std::mem::replace(&mut self.functions_to_analyze, vec![]);
        let interesting_fn_defs = self
            .marked_objects
            .iter()
            .filter_map(|(s, v)| match v.1 {
                ObjectType::Function(i) => {
                    Some((tcx.hir().local_def_id(*s).to_def_id(), (v.0.clone(), i)))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let mut call_site_annotations: CallSiteAnnotations = HashMap::new();
        crate::sah::HashVerifications::with(|hash_verifications| {
            targets
                .drain(..)
                .map(|(id, b, _)| {
                    self.handle_target(
                        hash_verifications,
                        &mut call_site_annotations,
                        &interesting_fn_defs,
                        id,
                        b,
                    )
                })
                .collect::<std::io::Result<HashMap<Endpoint, Ctrl>>>()
                .map(|controllers| ProgramDescription {
                    controllers,
                    annotations: call_site_annotations
                        .into_iter()
                        .map(|(k, v)| (identifier_for_fn(tcx, k), (v.0, ObjectType::Function(v.1))))
                        .chain(
                            self.marked_objects
                                .iter()
                                .filter(|kv| kv.1 .1 == ObjectType::Type)
                                .map(|(k, v)| (identifier_for_hid(tcx, *k), v.clone())),
                        )
                        .collect(),
                })
        })
    }

    /// XXX: This selector is somewhat problematic. For one it matches via
    /// source locations, rather than id, and for another we're using `find`
    /// here, which would discard additional matching annotations.
    fn statement_anns_by_loc(&self, p: DefId, t: &mir::Terminator<'tcx>) -> Option<&[Annotation]> {
        self.marked_stmts
            .iter()
            .find(|(_, (_, s, f))| p == *f && s.contains(t.source_info.span))
            .map(|t| t.1 .0 .0.as_slice())
    }
}

pub fn extract_places<'tcx>(
    l: mir::Location,
    body: &mir::Body<'tcx>,
    exclude_return_places_from_call: bool,
) -> HashSet<mir::Place<'tcx>> {
    use mir::visit::Visitor;
    let mut places = HashSet::new();
    let mut vis = PlaceVisitor(|p: &mir::Place<'tcx>| {
        places.insert(*p);
    });
    match body.stmt_at(l) {
        Either::Right(mir::Terminator {
            kind: mir::TerminatorKind::Call { func, args, .. },
            ..
        }) if exclude_return_places_from_call => std::iter::once(func)
            .chain(args.iter())
            .for_each(|o| vis.visit_operand(o, l)),
        _ => body.basic_blocks()[l.block]
            .visitable(l.statement_index)
            .apply(l, &mut vis),
    };
    places
}

use flowistry::indexed::{IndexMatrix, IndexSet};

pub type NonTransitiveGraph<'tcx> =
    HashMap<mir::Location, IndexMatrix<mir::Place<'tcx>, mir::Location>>;

fn is_safe_function<'tcx>(_bound_sig: &ty::Binder<'tcx, ty::FnSig<'tcx>>) -> bool {
    return false;
}

fn identifier_for_hid<'tcx>(tcx: TyCtxt<'tcx>, hid: HirId) -> Identifier {
    Identifier::new(tcx.item_name(tcx.hir().local_def_id(hid).to_def_id()))
}

fn identifier_for_fn<'tcx>(tcx: TyCtxt<'tcx>, p: DefId) -> Identifier {
    Identifier::new(tcx.item_name(p))
}

fn obj_type_for_stmt_ann(anns: &[Annotation]) -> usize {
    *anns
        .iter()
        .flat_map(|a| match a {
            Annotation::Label(LabelAnnotation { refinement, .. }) => {
                Box::new(refinement.on_argument().iter()) as Box<dyn Iterator<Item = &u16>>
            }
            Annotation::Exception(_) => Box::new(std::iter::once(&0)),
            _ => panic!("Unsupported annotation type for statement annotation"),
        })
        .max()
        .unwrap() as usize
}

impl<'tcx> intravisit::Visitor<'tcx> for Visitor<'tcx> {
    type NestedFilter = OnlyBodies;

    fn nested_visit_map(&mut self) -> Self::Map {
        self.tcx.hir()
    }

    /// Checks for annotations on this id and collects all those id's that have
    /// been annotated.
    fn visit_id(&mut self, id: HirId) {
        let tcx = self.tcx;
        let hir = self.tcx.hir();
        let sink_matches = hir
            .attrs(id)
            .iter()
            .filter_map(|a| {
                a.match_extract(&crate::LABEL_MARKER, |i| {
                    Annotation::Label(crate::ann_parse::ann_match_fn(i))
                })
                .or_else(|| {
                    a.match_extract(&crate::OTYPE_MARKER, |i| {
                        Annotation::OType(crate::ann_parse::otype_ann_match(i))
                    })
                })
                .or_else(|| {
                    a.match_extract(&crate::EXCEPTION_MARKER, |i| {
                        Annotation::Exception(crate::ann_parse::match_exception(i))
                    })
                })
            })
            .collect::<Vec<_>>();
        if !sink_matches.is_empty() {
            let node = self.tcx.hir().find(id).unwrap();
            assert!(if let Some(decl) = node.fn_decl() {
                self.marked_objects
                    .insert(id, (sink_matches, ObjectType::Function(decl.inputs.len())))
                    .is_none()
            } else {
                match node {
                    hir::Node::Ty(_)
                    | hir::Node::Item(hir::Item {
                        kind: hir::ItemKind::Struct(..),
                        ..
                    }) => self
                        .marked_objects
                        .insert(id, (sink_matches, ObjectType::Type))
                        .is_none(),
                    _ => {
                        let e = match node {
                            hir::Node::Expr(e) => e,
                            hir::Node::Stmt(hir::Stmt { kind, .. }) => match kind {
                                hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) => e,
                                _ => panic!("Unsupported statement kind"),
                            },
                            _ => panic!("Unsupported object type for annotation {node:?}"),
                        };
                        let obj_type = obj_type_for_stmt_ann(&sink_matches);
                        let did = match e.kind {
                            hir::ExprKind::MethodCall(_, _, _) => {
                                let body_id = hir.enclosing_body_owner(id);
                                let tcres = tcx.typeck(hir.local_def_id(body_id));
                                tcres.type_dependent_def_id(e.hir_id).unwrap_or_else(|| {
                                    panic!("No DefId found for method call {e:?}")
                                })
                            }
                            hir::ExprKind::Call(
                                hir::Expr {
                                    hir_id,
                                    kind: hir::ExprKind::Path(p),
                                    ..
                                },
                                _,
                            ) => {
                                let body_id = hir.enclosing_body_owner(id);
                                let tcres = tcx.typeck(hir.local_def_id(body_id));
                                match tcres.qpath_res(p, *hir_id) {
                                    hir::def::Res::Def(_, did) => did,
                                    res => panic!("Not a function? {res:?}"),
                                }
                            }
                            _ => panic!("Unsupported expression kind {:?}", e.kind),
                        };
                        self.marked_stmts
                            .insert(id, ((sink_matches, obj_type), e.span, did))
                            .is_none()
                    }
                }
            })
        }
    }

    /// Finds the functions that have been marked as targets.
    fn visit_fn(
        &mut self,
        fk: FnKind<'tcx>,
        fd: &'tcx rustc_hir::FnDecl<'tcx>,
        b: BodyId,
        s: Span,
        id: HirId,
    ) {
        match &fk {
            FnKind::ItemFn(ident, _, _) | FnKind::Method(ident, _)
                if self.should_analyze_function(id) =>
            {
                self.functions_to_analyze.push((*ident, b, fd));
            }
            _ => (),
        }

        // dispatch to recursive walk. This is probably unnecessary but if in
        // the future we decide to do something with nested items we may need
        // it.
        intravisit::walk_fn(self, fk, fd, b, s, id)
    }
}
