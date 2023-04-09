//! The place algebra
//!
//! This module defines the algebra for reasoning about relations of
//! abstract locations in memory.
//!
//! To run [`solve`], which can tell you how two memory locations relate, you
//! need a fact base made up of a set of [`Equality`] equations. Equations
//! comprise of [`Term`]s which in turn are a base with [`Operator`]s layered
//! around.
//!
//! For instance to extract a fact base from an MIR body use
//! [`extract_equations`].

use petgraph::visit::IntoEdges;

use crate::{
    either::Either,
    ir::regal::TargetPlace,
    mir::{self, Field, Local, Place},
    utils::{outfile_pls, write_sep, DisplayViaDebug, Print},
    HashMap, HashSet, Symbol, TyCtxt,
};

use std::{
    fmt::{Debug, Display, Write},
    hash::{Hash, Hasher},
};

/// Terms in the projection algebra
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Term<B, F: Copy> {
    /// The base of the term
    base: B,
    /// Operators applied to the term (in reverse order)
    terms: Vec<Operator<F>>,
}

fn display_term_pieces<F: Display + Copy, B: Display>(
    f: &mut std::fmt::Formatter<'_>,
    terms: &[Operator<F>],
    base: &B,
) -> std::fmt::Result {
    use Operator::*;
    for t in terms.iter().rev() {
        match t {
            RefOf => f.write_str("&("),
            DerefOf => f.write_str("*("),
            ContainsAt(field) => write!(f, "{{ .{}: ", field),
            Upcast(_, s) => write!(f, "(#{s}"),
            Unknown => write!(f, "(?"),
            _ => f.write_char('('),
        }?
    }
    write!(f, "{}", base)?;
    for t in terms.iter() {
        match t {
            MemberOf(field) => write!(f, ".{})", field),
            ContainsAt(_) => f.write_str(" }"),
            Downcast(_, s) => write!(f, " #{s})"),
            _ => f.write_char(')'),
        }?
    }
    Ok(())
}

impl<B: Display, F: Display + Copy> Display for Term<B, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        display_term_pieces(f, &self.terms, &self.base)
    }
}

impl Display for TargetPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetPlace::Argument(i) => write!(f, "a{}", i.as_usize()),
            TargetPlace::Return => f.write_char('r'),
        }
    }
}

type VariantIdx = usize;

/// An operator in the projection algebra.
#[derive(Clone, Eq, Hash, Debug, Copy, PartialEq)]
pub enum Operator<F: Copy> {
    RefOf,
    DerefOf,
    MemberOf(F),
    ContainsAt(F),
    Downcast(Option<Symbol>, VariantIdx),
    Upcast(Option<Symbol>, VariantIdx),
    Unknown,
}

/// Relationship of two [`Operator`]s. Used in [`Operator::cancel`].
#[derive(Clone, PartialEq, Eq, Hash, Debug, Copy)]
pub enum Cancel<F> {
    /// Both operators were field-related but did not reference the same field
    NonOverlappingField(F, F),
    /// Both operators were variant cast related but did not reference the same variant
    NonOverlappingVariant(VariantIdx, VariantIdx),
    /// The operators canceled
    CancelBoth,
    CancelOne,
    /// The operators did not cancel
    Remains,
}

impl<F: Copy> Operator<F> {
    /// Each operator has a dual, this flips this operator to that respective dual.
    pub fn flip(self) -> Self {
        use Operator::*;
        match self {
            RefOf => DerefOf,
            DerefOf => RefOf,
            MemberOf(f) => ContainsAt(f),
            ContainsAt(f) => MemberOf(f),
            Downcast(s, v) => Upcast(s, v),
            Upcast(s, v) => Downcast(s, v),
            Unknown => Unknown,
        }
    }

    pub fn is_unknown(self) -> bool {
        matches!(self, Operator::Unknown)
    }

    /// Determine for two term segments whether they cancel each other (for
    /// instance `*&x => x`) or not. It also reports if the two segments do not
    /// unify, which can be the case for fields and variant casts.
    ///
    /// I've been thinking about this and I think for fields the order here
    /// might actually matter. (And I think it would still be reorder safe).
    /// Say you do `a.f = b.g`. This statement is perfectly valid and it makes
    /// sense. If you reorder it you get `a = { .f: b.g }` and that (currently)
    /// cancels with `NonOverlappingField` because you get `ContainsAt(.f,
    /// MemberOf(b, .g))`.
    ///
    /// In the opposite case you have something like `a = { g: b }.f` this is
    /// obviously nonsense and not present in surface syntax but can be the
    /// result of substitution for instance for `x.g = b; a = x.f`. There will
    /// probably be other equations that describe what happens at `x.f` but this
    /// particular one when substituted is obviously useless. However note the
    /// order here is different. This is `MemberOf(ContainsAt(.g, b), .f)`. This
    /// one should eliminate.
    ///
    /// I had one fear about this which is "what happens when you reorder to the
    /// other side, doesn't the order change from the first one to the second?"
    /// turns out its fine, because the reordering will flip both segments and
    /// thus maintain the order. This is why I think adding this is not just
    /// safe but actually more sound.
    pub fn cancel(self, other: Self) -> Cancel<F>
    where
        F: PartialEq,
    {
        use Operator::*;
        match (self, other) {
            (Unknown, Unknown) => Cancel::CancelOne,
            (Unknown, _) | (_, Unknown) => Cancel::Remains,
            (MemberOf(f), ContainsAt(g)) | (ContainsAt(g), MemberOf(f)) if f != g => {
                Cancel::NonOverlappingField(f, g)
            }
            (Downcast(_, v1), Upcast(_, v2)) | (Upcast(_, v2), Downcast(_, v1)) if v1 != v2 => {
                Cancel::NonOverlappingVariant(v1, v2)
            }
            _ if self == other.flip() => Cancel::CancelOne,
            _ => Cancel::Remains,
        }
    }

    /// Apply a function to the field, creating a new operator
    pub fn map_field<F0: Copy, G: FnMut(F) -> F0>(self, mut g: G) -> Operator<F0> {
        use Operator::*;
        match self {
            RefOf => RefOf,
            DerefOf => DerefOf,
            MemberOf(f) => MemberOf(g(f)),
            ContainsAt(f) => ContainsAt(g(f)),
            Upcast(s, v) => Upcast(s, v),
            Downcast(s, v) => Downcast(s, v),
            Unknown => Unknown,
        }
    }
}

/// An equation in the algebra
#[derive(Clone, Debug)]
pub struct Equality<B, F: Copy> {
    lhs: Term<B, F>,
    rhs: Term<B, F>,
}

impl<B: Display, F: Display + Copy> Display for Equality<B, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} = {}", self.lhs, self.rhs)
    }
}

/// The Eq instance is special, because it is order independent with respect
/// to the left and right hand side.
impl<B: std::cmp::PartialEq, F: std::cmp::PartialEq + Copy> std::cmp::PartialEq for Equality<B, F> {
    fn eq(&self, other: &Self) -> bool {
        // Using an unpack here so compiler warns in case a new field is ever added
        let Equality { lhs, rhs } = other;
        (lhs == &self.lhs && rhs == &self.rhs) || (rhs == &self.lhs && lhs == &self.rhs)
    }
}

impl<B: Eq, F: Eq + Copy> Eq for Equality<B, F> {}

/// The Hash instance is special, because it is order independent with respect
/// to the left and right hand side.
impl<B: Hash, F: Hash + Copy> Hash for Equality<B, F> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut l = std::collections::hash_map::DefaultHasher::new();
        let mut r = std::collections::hash_map::DefaultHasher::new();

        self.lhs.hash(&mut l);
        self.rhs.hash(&mut r);

        state.write_u64(l.finish().wrapping_add(r.finish()))
    }
}

impl<B, F: Copy> Equality<B, F> {
    /// Create a new equation
    pub fn new(lhs: Term<B, F>, rhs: Term<B, F>) -> Self {
        Self { lhs, rhs }
    }

    /// Rearrange the equation, moving all operators from the left hand side to
    /// the right hand side term, [`Operator::flip`]ing them in the process.
    ///
    /// After calling this function it is guaranteed that `self.lhs.is_base() == true`
    ///
    /// If you want to rearrange from right to left use [`Equality::swap`]
    pub fn rearrange_left_to_right(&mut self) {
        self.rhs
            .terms
            .extend(self.lhs.terms.drain(..).rev().map(Operator::flip));
    }

    /// Swap the left and right hand side terms
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.lhs, &mut self.rhs)
    }

    pub fn bases(&self) -> [&B; 2] {
        [self.lhs.base(), self.rhs.base()]
    }

    /// Apply a function to each base, creating a new equation with a
    /// potentially different base type.
    pub fn map_bases<B0, G: FnMut(&B) -> B0>(&self, mut f: G) -> Equality<B0, F> {
        Equality {
            lhs: self.lhs.replace_base(f(self.lhs.base())),
            rhs: self.rhs.replace_base(f(self.rhs.base())),
        }
    }
}

/// A heavy lifter. This is a partial solver. Given a fact base (set of
/// equations) and a way to convert from the type of the base `B` to a new base
/// `N` this function will substitute, expand and simplify the entire fact base
/// to a new fact base with the new base type.
///
/// When considering any equation the bases are `inspect`ed. If it converts to a
/// new base `N` it will remain untouched, if it converts to a variable `V` the
/// variable will be substituted with each other equation that mentions the same
/// variable. This process continues until a newly substituted term's base is
/// not a variable. If there are no other equations for a given variable the
/// equation is abandoned. Variables are not recursively expanded to themselves.
pub fn rebase_simplify<
    GetEq: std::borrow::Borrow<Equality<B, F>>,
    NIt: IntoIterator<Item = N>,
    I: Fn(&B) -> Either<NIt, V>,
    It: Iterator<Item = GetEq>,
    N: Display + Clone,
    B: Clone + Hash + Eq + Display,
    F: Eq + Hash + Clone + Copy + Display,
    V: Clone + Eq + Hash + Display,
>(
    equations: It,
    inspect: I,
) -> Vec<Equality<N, F>> {
    let mut finals = vec![];
    let mut add_final = |mut eq: Equality<_, _>| {
        eq.rearrange_left_to_right();
        if eq.rhs.simplify() {
            finals.push(eq);
        }
    };

    let mut handle_eq = |mut eq: Equality<_, _>,
                         add_intermediate: &mut dyn FnMut(V, Term<_, _>)| {
        let il = inspect(eq.lhs.base());
        let ir = inspect(eq.rhs.base());
        if il.is_left() && ir.is_left() {
            let rv = ir.left().unwrap().into_iter().collect::<Vec<_>>();
            for newl in il.left().unwrap() {
                for newr in rv.iter() {
                    add_final(Equality {
                        lhs: eq.lhs.replace_base(newl.clone()),
                        rhs: eq.rhs.replace_base(newr.clone()),
                    });
                }
            }
        } else {
            if let Either::Right(v) = il {
                let mut eq_clone = eq.clone();
                eq_clone.rearrange_left_to_right();
                assert!(eq_clone.lhs.is_base());
                add_intermediate(v, eq_clone.rhs);
            }
            if let Either::Right(v) = ir {
                eq.swap();
                eq.rearrange_left_to_right();
                assert!(eq.lhs.is_base());
                add_intermediate(v, eq.rhs);
            }
        }
    };
    let mut queue = vec![];
    let mut intermediates: HashMap<V, HashSet<Term<B, F>>> = HashMap::default();
    let mut add_intermediate = |k: V, mut v: Term<_, _>| {
        if v.simplify() {
            intermediates
                .entry(k.clone())
                .or_insert_with(|| {
                    queue.push(k);
                    HashSet::default()
                })
                .insert(v);
        }
    };
    for eq in equations {
        handle_eq(eq.borrow().clone(), &mut add_intermediate);
    }
    debug!("Found {} intermediates", intermediates.len());
    // debug!(
    //     "Found the intermediates\n{}",
    //     crate::utils::Print(|f: &mut std::fmt::Formatter<'_>| {
    //         for (k, v) in intermediates.iter() {
    //             write!(f, "  {k}: ")?;
    //             let mut first = true;
    //             for t in v {
    //                 if first {
    //                     first = false;
    //                 } else {
    //                     f.write_str(", ")?;
    //                 }
    //                 t.fmt(f)?;
    //             }
    //             writeln!(f)?;
    //         }
    //         Ok(())
    //     })
    // );
    while let Some(v) = queue.pop() {
        let terms = intermediates.remove(&v).unwrap();
        debug!(
            "handling {v} ({} terms, {} combinations)",
            terms.len(),
            terms.len() * terms.len() / 2
        );
        if terms.len() > 10000 {
            info!(
                "Found more than 10000 terms, some of them are\n{}",
                Print(|f: &mut std::fmt::Formatter<'_>| {
                    write_sep(f, "\n", terms.iter().take(100), |elem, f| {
                        write!(f, "{}", elem)
                    })
                })
            )
        }
        // if terms.len() < 2 {
        //     debug!(
        //         "Found fewer than two terms for {v}: {}",
        //         Print(|f: &mut std::fmt::Formatter<'_>| {
        //             let mut first = true;
        //             for t in terms.iter() {
        //                 if first {
        //                     first = false;
        //                 } else {
        //                     f.write_str(", ")?;
        //                 }
        //                 t.fmt(f)?;
        //             }
        //             Ok(())
        //         })
        //     );
        // }
        for (idx, lhs) in terms.iter().enumerate() {
            for rhs in terms.iter().skip(idx + 1).cloned() {
                let eq = Equality {
                    lhs: lhs.clone(),
                    rhs,
                };
                handle_eq(eq, &mut |v, mut term| {
                    if let Some(s) = intermediates.get_mut(&v) {
                        if term.simplify() {
                            s.insert(term);
                        }
                    } else {
                        //debug!("Abandoning term {term} because {v} is already handled");
                    }
                });
            }
        }
    }

    finals
}

struct MemoEdge<F: Copy>(Vec<Vec<Operator<F>>>);

fn partial_cmp_terms<'a, F: Copy + Eq>(
    mut left: &'a [Operator<F>],
    mut right: &'a [Operator<F>],
) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    let greater = left.len() > right.len();
    if !greater {
        std::mem::swap(&mut left, &mut right);
    }
    let mut matches = false;
    for i in 0..(left.len() - right.len()) {
        if left[i..].iter().zip(right.iter()).all(|(l, r)| l == r) {
            matches = true;
            break;
        }
    }
    if !matches {
        None
    } else {
        Some(if left.len() == right.len() {
            Equal
        } else if greater {
            Greater
        } else {
            Less
        })
    }
}

impl<F: Copy + Eq> MemoEdge<F> {
    fn insert(&mut self, e: Vec<Operator<F>>) -> bool {
        let mut insert = true;
        for i in self.0.iter() {
            match partial_cmp_terms(&i, &e) {
                Some(std::cmp::Ordering::Equal) | Some(std::cmp::Ordering::Less) => {
                    insert = false;
                    break;
                }
                _ => (),
            }
        }
        if insert {
            self.0.push(e);
        }
        insert
    }
}

impl<F: Copy> Default for MemoEdge<F> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<F: Copy + Display> Display for MemoEdge<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_char('{')?;
        write_sep(f, ", ", self.0.iter(), |elem, f| {
            display_term_pieces(f, elem, &0)
        })?;
        f.write_char('}')
    }
}

type MemoizedSolutionImpl<B, F> = petgraph::prelude::GraphMap<B, MemoEdge<F>, petgraph::Directed>;
pub struct MemoizedSolution<B, F: Copy> {
    graph: MemoizedSolutionImpl<B, F>,
    on_demand: bool,
}

impl<B: petgraph::graphmap::NodeTrait + Eq + Display + Debug, F: Eq + Hash + Copy + Display>
    MemoizedSolution<B, F>
{
    fn insert_edge(
        graph: &mut MemoizedSolutionImpl<B, F>,
        from: B,
        to: B,
        delta: Vec<Operator<F>>,
    ) -> bool {
        if graph.contains_edge(to, from) {
            graph
                .edge_weight_mut(to, from)
                .unwrap()
                .insert(delta.into_iter().map(|o| o.flip()).collect())
        } else {
            if !graph.contains_edge(from, to) {
                graph.add_edge(from, to, Default::default());
            }
            graph.edge_weight_mut(from, to).unwrap().insert(delta)
        }
    }
    pub fn reachable(&self, from: B, to: B) -> bool {
        if self.on_demand {
            for path in
                petgraph::algo::all_simple_paths::<Vec<_>, _>(&self.graph, from, to, 0, None)
            {
                let mut it = path.into_iter().peekable();
                let mut prior_terms = vec![Term::new_base(0)];
                let mut next_terms = vec![];
                while let Some(this) = it.next() {
                    if let Some(next) = it.peek() {
                        let iters = self
                            .graph
                            .edge_weight(this, *next)
                            .into_iter()
                            .map(|e| (e, false))
                            .chain(
                                self.graph
                                    .edge_weight(*next, this)
                                    .into_iter()
                                    .map(|e| (e, true)),
                            )
                            .collect::<Vec<_>>();
                        for (weights, flip) in iters {
                            for w in &weights.0[1..] {
                                for p in &prior_terms {
                                    let mut c = p.clone();
                                    if flip {
                                        c.terms.extend(w.iter().cloned().map(Operator::flip))
                                    } else {
                                        c.terms.extend(w)
                                    }
                                    if c.simplify() {
                                        next_terms.push(c)
                                    }
                                }
                            }
                            if let Some(w) = weights.0.get(0) {
                                prior_terms.retain_mut(|t| {
                                    if flip {
                                        t.terms.extend(w.iter().cloned().map(Operator::flip));
                                    } else {
                                        t.terms.extend(w);
                                    }
                                    t.simplify()
                                })
                            }
                        }
                        prior_terms.extend(next_terms.drain(..));
                    }
                }
                if !prior_terms.is_empty() {
                    return true;
                }
            }
            false
        } else {
            self.graph
                .edge_weight(from, to)
                .map(|e| (&e.0, true))
                .or_else(|| self.graph.edge_weight(to, from).map(|e| (&e.0, false)))
                .map_or(false, |e| !e.0.is_empty())
        }
    }

    fn initial<I: Iterator<Item = Equality<B, F>>>(it: I) -> MemoizedSolutionImpl<B, F> {
        let mut graph: MemoizedSolutionImpl<B, F> = petgraph::prelude::GraphMap::default();

        for mut eq in it {
            let rn = *eq.rhs.base();
            let ln = *eq.lhs.base();
            eq.rearrange_left_to_right();
            Self::insert_edge(&mut graph, ln, rn, eq.rhs.terms);
        }
        graph
    }

    fn construct_on_demand<I: Iterator<Item = Equality<B, F>>>(it: I) -> Self {
        Self {
            graph: Self::initial(it),
            on_demand: true,
        }
    }

    pub fn construct<I: Iterator<Item = Equality<B, F>>>(it: I) -> Self {
        let mut graph = Self::initial(it);
        let mut queue = graph.nodes().collect::<HashSet<_>>();

        while let Some(middle) = queue.iter().next().cloned().map(|elem| {
            queue.remove(&elem);
            elem
        }) {
            let g_ref = &graph;
            let new_edges = graph
                .edges(middle)
                .flat_map(|(in_from, in_to, from_weights)| {
                    let (from, from_flip_needed) = if in_to == middle {
                        (in_from, false)
                    } else {
                        assert_eq!(in_from, middle);
                        (in_to, true)
                    };
                    if in_from == in_to || from == middle {
                        return Either::Right(std::iter::empty());
                    }
                    Either::Left(from_weights.0.iter().flat_map(move |from_weight| {
                        g_ref
                            .edges(middle)
                            .flat_map(move |(out_from, out_to, to_weights)| {
                                let (next, to_flip_needed) = if out_from == middle {
                                    (out_to, false)
                                } else {
                                    assert_eq!(out_to, middle);
                                    (out_from, true)
                                };
                                if out_from == out_to || next == middle {
                                    return Either::Right(std::iter::empty());
                                }
                                Either::Left(to_weights.0.iter().filter_map(move |to_weight| {
                                    let mut new_terms = if from_flip_needed {
                                        from_weight.iter().cloned().map(Operator::flip).collect()
                                    } else {
                                        from_weight.clone()
                                    };
                                    if to_flip_needed {
                                        new_terms.extend(to_weight.iter().map(|e| e.flip()));
                                    } else {
                                        new_terms.extend(to_weight);
                                    };
                                    let mut t = Term::from_raw(DisplayViaDebug(()), new_terms);
                                    t.simplify().then_some((from, next, t.terms))
                                }))
                            })
                    }))
                })
                .collect::<Vec<_>>();
            for (from, last, terms) in new_edges {
                if Self::insert_edge(&mut graph, from, last, terms.clone()) {
                    debug!(
                        "Adding edge {from} -> {last} with {}",
                        Term::from_raw(0, terms)
                    );
                    queue.extend([from, last]);
                }
            }
        }
        Self {
            graph,
            on_demand: false,
        }
    }
}

pub fn dump_dot_graph<
    B: Display + petgraph::graphmap::NodeTrait,
    F: Display + Copy,
    W: std::io::Write,
>(
    mut w: W,
    g: &MemoizedSolution<B, F>,
) -> std::io::Result<()> {
    use petgraph::dot::*;
    write!(
        w,
        "{}",
        Dot::with_attr_getters(&g.graph, &[], &|_, _| "".to_string(), &|_, _| "shape=box"
            .to_string(),)
    )
}

/// Solve for the relationship of two bases.
///
/// Returns all terms `t` such that `from = t(to)`. If no terms are returned the
/// two bases are not related (memory non interference).
///
/// If you need to instead solve for the relationship of two terms `t1`, `t2`, generate two
/// new bases `x`, `y` then extend the fact base with the equations `x = t1`,
/// `y = t2` and solve for `x` and `y` instead.
///
pub fn solve<
    B: Clone + Hash + Eq + Display,
    F: Eq + Hash + Clone + Copy + Display,
    GetEq: std::borrow::Borrow<Equality<B, F>>,
>(
    equations: &[GetEq],
    from: &B,
    to: &B,
) -> Vec<Vec<Operator<F>>> {
    let mut solutions = vec![];
    solve_with(
        equations,
        from,
        |found| found == to,
        |solution| {
            solutions.push(solution);
            true
        },
    );
    solutions
}

pub fn solve_reachable<
    B: Clone + Hash + Eq + Display,
    F: Eq + Hash + Clone + Copy + Display,
    GetEq: std::borrow::Borrow<Equality<B, F>>,
    IsTarget: FnMut(&B) -> bool,
>(
    equations: &[GetEq],
    from: &B,
    to: IsTarget,
) -> bool {
    let mut reachable = false;
    solve_with(equations, from, to, |solution| {
        reachable = true;
        false
    });
    reachable
}

fn solve_with<
    B: Clone + Hash + Eq + Display,
    F: Eq + Hash + Clone + Copy + Display,
    GetEq: std::borrow::Borrow<Equality<B, F>>,
    RegisterFinal: FnMut(Vec<Operator<F>>) -> bool,
    IsTarget: FnMut(&B) -> bool,
>(
    equations: &[GetEq],
    from: &B,
    mut is_target: IsTarget,
    mut register_final: RegisterFinal,
) {
    if is_target(from) {
        register_final(vec![]);
        return;
    }
    let mut eqs_with_bases = equations
        .iter()
        .map(|e| {
            (
                e.borrow().bases().into_iter().collect::<Vec<_>>(),
                e.borrow(),
            )
        })
        .collect::<Vec<_>>();
    let mut intermediates: HashMap<B, HashSet<Term<B, F>>> = HashMap::new();
    let mut find_matching = |target: &B| {
        eqs_with_bases
            .drain_filter(|(bases, _eq)| bases.contains(&target))
            .map(|(_, eq)| eq)
            .collect::<Vec<_>>()
    };

    let mut targets = vec![from.clone()];

    while let Some(intermediate_target) = targets.pop() {
        if intermediates.contains_key(&intermediate_target) {
            continue;
        }
        let all_matching = find_matching(&intermediate_target);
        // if all_matching.is_empty() {
        //     debug!(
        //         "No matching equation for intermediate target {} from {}",
        //         intermediate_target, from
        //     );
        // }
        for mut matching in all_matching.into_iter().cloned() {
            if matching.lhs.base() != &intermediate_target {
                matching.swap()
            }
            matching.rearrange_left_to_right();
            if !is_target(matching.rhs.base()) {
                targets.push(matching.rhs.base().clone());
            }
            intermediates
                .entry(intermediate_target.clone())
                .or_insert_with(HashSet::default)
                .insert(matching.rhs);
        }
    }
    debug!("Found {} intermedaites", intermediates.len());
    // debug!("Found the intermediates");
    // for (k, vs) in intermediates.iter() {
    //     debug!(
    //         "  {k}: {}",
    //         Print(|f: &mut std::fmt::Formatter| {
    //             let mut first = true;
    //             for term in vs {
    //                 if first {
    //                     first = false;
    //                 } else {
    //                     f.write_str(" || ")?;
    //                 }
    //                 write!(f, "{}", term)?;
    //             }
    //             Ok(())
    //         })
    //     );
    // }
    let matching_intermediate = intermediates.get(from);
    if matching_intermediate.is_none() {
        debug!("No intermediate found for {from}");
    }
    let mut targets = matching_intermediate
        .into_iter()
        .flat_map(|v| v.iter().cloned())
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    while let Some(intermediate_target) = targets.pop() {
        let var = intermediate_target.base();
        if is_target(var) {
            if !register_final(intermediate_target.terms) {
                return;
            }
        } else if seen.contains(var) {
            //debug!("Aborting search on recursive visit to {var}")
        } else {
            seen.insert(var.clone());
            if let Some(next_eq) = intermediates.get(&var) {
                targets.extend(next_eq.iter().cloned().filter_map(|term| {
                    let mut to_sub = intermediate_target.clone();
                    to_sub.sub(term);
                    to_sub.simplify().then_some(to_sub)
                }))
            } else {
                //debug!("No follow up equation found for {var} on the way from {from}");
            }
        }
    }
}

fn vec_drop_range<T>(v: &mut Vec<T>, r: std::ops::Range<usize>) {
    let ptr = v.as_mut_ptr();
    for i in r.clone() {
        unsafe {
            drop(ptr.add(i))
        }
    }
    unsafe {
        std::ptr::copy(ptr.add(r.end), ptr.add(r.start), v.len() - r.end);
        v.set_len(v.len() - r.len());
    }
}

impl<B, F: Copy> Term<B, F> {
    pub fn is_base(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn new_base(base: B) -> Self {
        Term {
            base,
            terms: vec![],
        }
    }

    pub fn add_deref_of(mut self) -> Self {
        self.terms.push(Operator::DerefOf);
        self
    }

    pub fn add_ref_of(mut self) -> Self {
        self.terms.push(Operator::RefOf);
        self
    }

    pub fn add_member_of(mut self, field: F) -> Self {
        self.terms.push(Operator::MemberOf(field));
        self
    }

    pub fn add_contains_at(mut self, field: F) -> Self {
        self.terms.push(Operator::ContainsAt(field));
        self
    }

    pub fn add_downcast(mut self, symbol: Option<Symbol>, idx: VariantIdx) -> Self {
        self.terms.push(Operator::Downcast(symbol, idx));
        self
    }

    pub fn add_upcast(mut self, symbol: Option<Symbol>, idx: VariantIdx) -> Self {
        self.terms.push(Operator::Upcast(symbol, idx));
        self
    }

    pub fn add_unknown(mut self) -> Self {
        self.terms.push(Operator::Unknown);
        self
    }

    pub fn base(&self) -> &B {
        &self.base
    }

    pub fn sub(&mut self, other: Self) {
        let Self { base, mut terms } = other;
        self.base = base;
        terms.append(&mut self.terms);
        std::mem::swap(&mut self.terms, &mut terms)
    }

    pub fn simplify(&mut self) -> bool
    where
        F: Eq + Display,
        B: Display,
    {
        let l = self.terms.len();
        let old_terms = std::mem::replace(&mut self.terms, Vec::with_capacity(l));
        let mut it = old_terms.into_iter().peekable();
        let mut valid = true;
        let mut after_first_unknown = None;
        let mut after_last_unknown = None;
        while let Some(i) = it.next() {
            if let Some(next) = it.peek().cloned() {
                match i.cancel(next) {
                    Cancel::NonOverlappingField(f, g) => {
                        valid = false;
                    }
                    Cancel::NonOverlappingVariant(v1, v2) => {
                        valid = false;
                    }
                    Cancel::CancelBoth => {
                        it.next();
                        continue;
                    }
                    Cancel::CancelOne => {
                        continue;
                    }
                    _ => (),
                }
            }
            self.terms.push(i);
            if i.is_unknown() {
                if after_first_unknown.is_none() {
                    &mut after_first_unknown
                } else {
                    &mut after_last_unknown
                }.insert(self.terms.len());
            }
        }
        if let (Some(from), Some(to)) = (after_first_unknown, after_last_unknown) {
            vec_drop_range(&mut self.terms, from..to);
        }
        valid
    }

    pub fn replace_base<B0>(&self, base: B0) -> Term<B0, F> {
        Term {
            base,
            terms: self.terms.clone(),
        }
    }

    pub fn replace_fields<F0: Copy, G: FnMut(F) -> F0>(&self, mut g: G) -> Term<B, F0>
    where
        B: Clone,
    {
        Term {
            base: self.base.clone(),
            terms: self.terms.iter().map(|f| f.map_field(&mut g)).collect(),
        }
    }

    pub fn from_raw(base: B, terms: Vec<Operator<F>>) -> Self {
        Self { base, terms }
    }
}

impl<B> Term<B, Field> {
    pub fn wrap_in_elem(self, elem: mir::PlaceElem) -> Self {
        use mir::ProjectionElem::*;
        match elem {
            Field(f, _) => self.add_member_of(f),
            Deref => self.add_deref_of(),
            Downcast(s, v) => self.add_downcast(s, v.as_usize()),
            _ => unimplemented!("{:?}", elem),
        }
    }
}

pub type MirEquation = Equality<DisplayViaDebug<Local>, DisplayViaDebug<Field>>;

struct Extractor<'tcx> {
    tcx: TyCtxt<'tcx>,
    equations: HashSet<MirEquation>,
}

impl<'tcx> Extractor<'tcx> {
    fn new(tcx: TyCtxt<'tcx>) -> Self {
        Self {
            tcx,
            equations: Default::default(),
        }
    }
}

type MirTerm = Term<DisplayViaDebug<Local>, DisplayViaDebug<Field>>;

impl From<Place<'_>> for MirTerm {
    fn from(p: Place<'_>) -> Self {
        let mut term = Term::new_base(DisplayViaDebug(p.local));
        for (_, proj) in p.iter_projections() {
            term = term.wrap_in_elem(proj);
        }
        term.replace_fields(DisplayViaDebug)
    }
}

impl From<&'_ Place<'_>> for MirTerm {
    fn from(p: &'_ Place<'_>) -> Self {
        MirTerm::from(*p)
    }
}

impl<'tcx> mir::visit::Visitor<'tcx> for Extractor<'tcx> {
    fn visit_assign(
        &mut self,
        place: &mir::Place<'tcx>,
        rvalue: &mir::Rvalue<'tcx>,
        _location: mir::Location,
    ) {
        let lhs = MirTerm::from(place);
        use mir::{AggregateKind, Rvalue::*};
        let rhs_s = match rvalue {
            Use(op) | UnaryOp(_, op) => Box::new(op.place().into_iter().map(|p| p.into()))
                as Box<dyn Iterator<Item = MirTerm>>,
            Ref(_, _, p) => {
                let term = MirTerm::from(p).add_ref_of();
                Box::new(std::iter::once(term)) as Box<_>
            }
            BinaryOp(_, box (op1, op2)) | CheckedBinaryOp(_, box (op1, op2)) => Box::new(
                [op1, op2]
                    .into_iter()
                    .flat_map(|op| op.place().into_iter())
                    .map(|op| op.into()),
            )
                as Box<_>,
            Aggregate(box kind, ops) => match kind {
                AggregateKind::Adt(def_id, idx, _, _, _) => {
                    let adt_def = self.tcx.adt_def(*def_id);
                    let variant = adt_def.variant(*idx);
                    let iter = variant
                        .fields
                        .iter()
                        .enumerate()
                        .zip(ops.iter())
                        .filter_map(|((i, _field), op)| {
                            let place = op.place()?;
                            // let field = mir::ProjectionElem::Field(
                            //     Field::from_usize(i),
                            //     field.ty(self.tcx, substs),
                            // );
                            Some(
                                MirTerm::from(place)
                                    .add_contains_at(DisplayViaDebug(Field::from_usize(i))),
                            )
                        });
                    Box::new(iter) as Box<_>
                }
                AggregateKind::Tuple => Box::new(ops.iter().enumerate().filter_map(|(i, op)| {
                    op.place()
                        .map(|p| MirTerm::from(p).add_contains_at(DisplayViaDebug(i.into())))
                })) as Box<_>,
                AggregateKind::Generator(_gen_id, _, _) => {
                    // I think this is the proper way to do this but the fields
                    // were sometimes empty and I don't know why so I'm doing
                    // the hacky thing below instead
                    // let gen_def =
                    // self.tcx.generator_layout(*gen_id).unwrap();
                    // debug!("variant fields {:?}", gen_def);
                    // let variant = gen_def.variant_fields.raw.first().unwrap();
                    // assert_eq!(variant.len(), ops.len());
                    // let it = variant.iter_enumerated().zip(ops).filter_map(|((field, _), op)| {
                    //     Some(MirTerm::from(op.place()?).add_contains_at(DisplayViaDebug(field)))
                    // });
                    let it = ops.iter().enumerate().filter_map(|(i, op)| {
                        Some(
                            MirTerm::from(op.place()?)
                                .add_contains_at(DisplayViaDebug(Field::from_usize(i))),
                        )
                    });
                    Box::new(it) as Box<_>
                }
                _ => {
                    debug!("Unhandled rvalue {rvalue:?}");
                    Box::new(std::iter::empty()) as Box<_>
                }
            },

            other => {
                debug!("Unhandled rvalue {other:?}");
                Box::new(std::iter::empty()) as Box<_>
            }
        };
        self.equations.extend(rhs_s.map(|rhs| Equality {
            lhs: lhs.clone(),
            rhs,
        }))
    }
}

/// Extract a fact base from the statements in an MIR body.
pub fn extract_equations<'tcx>(tcx: TyCtxt<'tcx>, body: &mir::Body<'tcx>) -> HashSet<MirEquation> {
    use mir::visit::Visitor;
    let mut extractor = Extractor::new(tcx);
    extractor.visit_body(body);
    extractor.equations
}
