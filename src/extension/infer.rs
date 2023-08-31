//! Inference for extension requirements on nodes of a hugr.
//!
//! Checks if the extensions requirements have a solution in terms of some
//! number of starting variables, and comes up with concrete solutions when
//! possible.
//!
//! Open extension variables can come from toplevel nodes: notionally "inputs"
//! to the graph where being wired up to a larger hugr would provide the
//! information needed to solve variables. When extension requirements of nodes
//! depend on these open variables, then the validation check for extensions
//! will succeed regardless of what the variable is instantiated to.

use super::{ExtensionId, ExtensionSet};
use crate::{hugr::views::HugrView, hugr::Node, ops::OpType, types::EdgeKind, Direction};

use super::validate::ExtensionError;

use petgraph::graph as pg;

use std::collections::{HashMap, HashSet};

use thiserror::Error;

/// A mapping from locations on the hugr to extension requirement sets which
/// have been inferred for them
pub type ExtensionSolution = HashMap<(Node, Direction), ExtensionSet>;

/// Infer extensions for a hugr. This is the main API exposed by this module
///
/// Return a tuple of the solutions found for locations on the graph, and a
/// closure: a solution which would be valid if all of the variables in the graph
/// were instantiated to an empty extension set. This is used (by validation) to
/// concretise the extension requirements of the whole hugr.
pub fn infer_extensions(
    hugr: &impl HugrView,
) -> Result<(ExtensionSolution, ExtensionSolution), InferExtensionError> {
    let mut ctx = UnificationContext::new(hugr);
    let solution = ctx.main_loop()?;
    ctx.instantiate_variables();
    let closed_solution = ctx.main_loop()?;
    let closure: HashMap<(Node, Direction), ExtensionSet> = closed_solution
        .into_iter()
        .filter(|(loc, _)| !solution.contains_key(loc))
        .collect();
    Ok((solution, closure))
}

/// Metavariables don't need much
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Meta(u32);

impl Meta {
    pub fn new(m: u32) -> Self {
        Meta(m)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
/// Things we know about metavariables
enum Constraint {
    /// A variable has the same value as another variable
    Equal(Meta),
    /// Variable extends the value of another by one extension
    Plus(ExtensionId, Meta),
}

#[derive(Debug, Clone, PartialEq, Error)]
/// Errors which arise during unification
pub enum InferExtensionError {
    #[error("Mismatched extension sets {expected} and {actual}")]
    /// We've solved a metavariable, then encountered a constraint
    /// that says it should be something other than our solution
    MismatchedConcrete {
        /// The solution we were trying to insert for this meta
        expected: ExtensionSet,
        /// The incompatible solution that we found was already there
        actual: ExtensionSet,
    },
    #[error("Solved extensions {expected} at {expected_loc:?} and {actual} at {actual_loc:?} should be equal.")]
    /// A version of the above with info about which nodes failed to unify
    MismatchedConcreteWithLocations {
        /// Where the solution we want to insert came from
        expected_loc: (Node, Direction),
        /// The solution we were trying to insert for this meta
        expected: ExtensionSet,
        /// Which node we're trying to add a solution for
        actual_loc: (Node, Direction),
        /// The incompatible solution that we found was already there
        actual: ExtensionSet,
    },
    /// A variable went unsolved that wasn't related to a parameter
    #[error("Unsolved variable at location {:?}", location)]
    Unsolved {
        /// The location on the hugr that's associated to the unsolved meta
        location: (Node, Direction),
    },
    /// An extension mismatch between two nodes which are connected by an edge.
    /// This should mirror (or reuse) `ValidationError`'s SrcExceedsTgtExtensions
    /// and TgtExceedsSrcExtensions
    #[error("Edge mismatch: {0}")]
    EdgeMismatch(#[from] ExtensionError),
}

/// A graph of metavariables which we've found equality constraints for. Edges
/// between nodes represent equality constraints.
struct EqGraph {
    equalities: pg::Graph<Meta, (), petgraph::Undirected>,
    node_map: HashMap<Meta, pg::NodeIndex>,
}

impl EqGraph {
    /// Create a new `EqGraph`
    fn new() -> Self {
        EqGraph {
            equalities: pg::Graph::new_undirected(),
            node_map: HashMap::new(),
        }
    }

    /// Add a metavariable to the graph as a node and return the `NodeIndex`.
    /// If it's already there, just return the existing `NodeIndex`
    fn add_or_retrieve(&mut self, m: Meta) -> pg::NodeIndex {
        self.node_map.get(&m).cloned().unwrap_or_else(|| {
            let ix = self.equalities.add_node(m);
            self.node_map.insert(m, ix);
            ix
        })
    }

    /// Create an edge between two nodes on the graph, declaring that they stand
    /// for metavariables which should be equal.
    fn register_eq(&mut self, src: Meta, tgt: Meta) {
        let src_ix = self.add_or_retrieve(src);
        let tgt_ix = self.add_or_retrieve(tgt);
        self.equalities.add_edge(src_ix, tgt_ix, ());
    }

    /// Return the connected components of the graph in terms of metavariables
    fn ccs(&self) -> Vec<Vec<Meta>> {
        petgraph::algo::tarjan_scc(&self.equalities)
            .into_iter()
            .map(|cc| {
                cc.into_iter()
                    .map(|n| *self.equalities.node_weight(n).unwrap())
                    .collect()
            })
            .collect()
    }
}

/// Our current knowledge about the extensions of the graph
struct UnificationContext {
    /// A list of constraints for each metavariable
    constraints: HashMap<Meta, HashSet<Constraint>>,
    /// A map which says which nodes correspond to which metavariables
    extensions: HashMap<(Node, Direction), Meta>,
    /// Solutions to metavariables
    solved: HashMap<Meta, ExtensionSet>,
    /// A graph which says which metavariables should be equal
    eq_graph: EqGraph,
    /// A mapping from metavariables which have been merged, to the meta they've
    // been merged to
    shunted: HashMap<Meta, Meta>,
    /// Variables we're allowed to include in solutionss
    variables: HashSet<Meta>,
    /// A name for the next metavariable we create
    fresh_name: u32,
}

/// Invariant: Constraint::Plus always points to a fresh metavariable
impl UnificationContext {
    /// Create a new unification context, and populate it with constraints from
    /// traversing the hugr which is passed in.
    pub fn new(hugr: &impl HugrView) -> Self {
        let mut ctx = Self {
            constraints: HashMap::new(),
            extensions: HashMap::new(),
            solved: HashMap::new(),
            eq_graph: EqGraph::new(),
            shunted: HashMap::new(),
            variables: HashSet::new(),
            fresh_name: 0,
        };
        ctx.gen_constraints(hugr);
        ctx
    }

    /// Create a fresh metavariable, and increment `fresh_name` for next time
    fn fresh_meta(&mut self) -> Meta {
        let fresh = Meta::new(self.fresh_name);
        self.fresh_name += 1;
        self.constraints.insert(fresh, HashSet::new());
        fresh
    }

    /// Declare a constraint on the metavariable
    fn add_constraint(&mut self, m: Meta, c: Constraint) {
        self.constraints
            .entry(m)
            .or_insert(HashSet::new())
            .insert(c);
    }

    /// Declare that a meta has been solved
    fn add_solution(&mut self, m: Meta, rs: ExtensionSet) {
        debug_assert!(self.solved.insert(m, rs).is_none());
    }

    /// If a metavariable has been merged, return the new meta, otherwise return
    /// the same meta.
    ///
    /// This could loop if there were a cycle in the `shunted` list, but there
    /// shouldn't be, because we only ever shunt to *new* metas.
    fn resolve(&self, m: Meta) -> Meta {
        self.shunted.get(&m).cloned().map_or(m, |m| self.resolve(m))
    }

    /// Get the relevant constraints for a metavariable. If it's been merged,
    /// get the constraints for the merged metavariable
    fn get_constraints(&self, m: &Meta) -> Option<&HashSet<Constraint>> {
        self.constraints.get(&self.resolve(*m))
    }

    /// Get the relevant solution for a metavariable. If it's been merged, get
    /// the solution for the merged metavariable
    fn get_solution(&self, m: &Meta) -> Option<&ExtensionSet> {
        self.solved.get(&self.resolve(*m))
    }

    /// Convert an extension *set* difference in terms of a sequence of fresh
    /// metas with `Plus` constraints which each add only one extension req.
    fn gen_union_constraint(&mut self, input: Meta, output: Meta, delta: ExtensionSet) {
        let mut last_meta = input;
        // Create fresh metavariables with `Plus` constraints for
        // each extension that should be added by the node
        // Hence a extension delta [A, B] would lead to
        // > ma = fresh_meta()
        // > add_constraint(ma, Plus(a, input)
        // > mb = fresh_meta()
        // > add_constraint(mb, Plus(b, ma)
        // > add_constraint(output, Equal(mb))
        for r in delta.0.into_iter() {
            let curr_meta = self.fresh_meta();
            self.add_constraint(curr_meta, Constraint::Plus(r, last_meta));
            last_meta = curr_meta;
        }
        self.add_constraint(output, Constraint::Equal(last_meta));
    }

    /// Return the metavariable corresponding to the given location on the
    /// graph, either by making a new meta, or looking it up
    fn make_or_get_meta(&mut self, node: Node, dir: Direction) -> Meta {
        if let Some(m) = self.extensions.get(&(node, dir)) {
            *m
        } else {
            let m = self.fresh_meta();
            self.extensions.insert((node, dir), m);
            m
        }
    }

    /// Iterate over the nodes in a hugr and generate unification constraints
    fn gen_constraints<T>(&mut self, hugr: &T)
    where
        T: HugrView,
    {
        if hugr.root_type().signature().is_none() {
            let m_input = self.make_or_get_meta(hugr.root(), Direction::Incoming);
            self.variables.insert(m_input);
        }

        for node in hugr.nodes() {
            let m_input = self.make_or_get_meta(node, Direction::Incoming);
            let m_output = self.make_or_get_meta(node, Direction::Outgoing);

            let node_type = hugr.get_nodetype(node);

            // Add constraints for the inputs and outputs of dataflow nodes according
            // to the signature of the parent node
            if let Some([input, output]) = hugr.get_io(node) {
                for dir in Direction::BOTH {
                    let m_input_node = self.make_or_get_meta(input, dir);
                    self.add_constraint(m_input_node, Constraint::Equal(m_input));
                    let m_output_node = self.make_or_get_meta(output, dir);
                    self.add_constraint(m_output_node, Constraint::Equal(m_output));
                }
            }

            match node_type.signature() {
                // Input extensions are open
                None => {
                    self.gen_union_constraint(
                        m_input,
                        m_output,
                        node_type.op_signature().extension_reqs,
                    );
                }
                // We have a solution for everything!
                Some(sig) => {
                    self.add_solution(m_output, sig.output_extensions());
                    self.add_solution(m_input, sig.input_extensions);
                }
            }
        }
        // Seperate loop so that we can assume that a metavariable has been
        // added for every (Node, Direction) in the graph already.
        for tgt_node in hugr.nodes() {
            let sig: &OpType = hugr.get_nodetype(tgt_node).into();
            // Incoming ports with a dataflow edge
            for port in hugr
                .node_inputs(tgt_node)
                .filter(|src_port| matches!(sig.port_kind(*src_port), Some(EdgeKind::Value(_))))
            {
                for (src_node, _) in hugr.linked_ports(tgt_node, port) {
                    let m_src = self
                        .extensions
                        .get(&(src_node, Direction::Outgoing))
                        .unwrap();
                    let m_tgt = self
                        .extensions
                        .get(&(tgt_node, Direction::Incoming))
                        .unwrap();
                    self.add_constraint(*m_src, Constraint::Equal(*m_tgt));
                }
            }
        }
    }

    /// When trying to unify two metas, check if they both correspond to
    /// different ends of the same wire. If so, return an `ExtensionError`.
    /// Otherwise check whether they both correspond to *some* location on the
    /// graph and include that info the otherwise generic `MismatchedConcrete`.
    fn report_mismatch(
        &self,
        m1: Meta,
        m2: Meta,
        rs1: ExtensionSet,
        rs2: ExtensionSet,
    ) -> InferExtensionError {
        let loc1 = self
            .extensions
            .iter()
            .find(|(_, m)| **m == m1 || self.resolve(**m) == m1)
            .map(|a| a.0);
        let loc2 = self
            .extensions
            .iter()
            .find(|(_, m)| **m == m2 || self.resolve(**m) == m2)
            .map(|a| a.0);
        let err = if let (Some((node1, dir1)), Some((node2, dir2))) = (loc1, loc2) {
            // N.B. We're looking for the case where an equality constraint
            // arose because the two locations are connected by an edge

            // If the directions are the same, they shouldn't be connected
            // to each other. If the nodes are the same, there's no edge!
            //
            // TODO: It's still possible that the equality constraint
            // arose because one node is a dataflow parent and the other
            // is one of it's I/O nodes. In that case, the directions could be
            // the same, and we should try to detect it
            if dir1 != dir2 && node1 != node2 {
                let [(src, src_rs), (tgt, tgt_rs)] = if *dir2 == Direction::Incoming {
                    [(node1, rs1.clone()), (node2, rs2.clone())]
                } else {
                    [(node2, rs2.clone()), (node1, rs1.clone())]
                };

                if src_rs.is_subset(&tgt_rs) {
                    Some(InferExtensionError::EdgeMismatch(
                        ExtensionError::TgtExceedsSrcExtensions {
                            from: *src,
                            from_extensions: src_rs,
                            to: *tgt,
                            to_extensions: tgt_rs,
                        },
                    ))
                } else {
                    Some(InferExtensionError::EdgeMismatch(
                        ExtensionError::SrcExceedsTgtExtensions {
                            from: *src,
                            from_extensions: src_rs,
                            to: *tgt,
                            to_extensions: tgt_rs,
                        },
                    ))
                }
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(loc1), Some(loc2)) = (loc1, loc2) {
            err.unwrap_or(InferExtensionError::MismatchedConcreteWithLocations {
                expected_loc: *loc1,
                expected: rs1,
                actual_loc: *loc2,
                actual: rs2,
            })
        } else {
            err.unwrap_or(InferExtensionError::MismatchedConcrete {
                expected: rs1,
                actual: rs2,
            })
        }
    }

    /// Take a group of equal metas and merge them into a new, single meta.
    ///
    /// Returns the set of new metas created and the set of metas that were
    /// merged.
    fn merge_equal_metas(&mut self) -> Result<(HashSet<Meta>, HashSet<Meta>), InferExtensionError> {
        let mut merged: HashSet<Meta> = HashSet::new();
        let mut new_metas: HashSet<Meta> = HashSet::new();
        for cc in self.eq_graph.ccs().into_iter() {
            // Within a connected component everything is equal
            let combined_meta = self.fresh_meta();
            for m in cc.iter() {
                // The same meta shouldn't be shunted twice directly. Only
                // transitively, as we still process the meta it was shunted to
                if self.shunted.contains_key(m) {
                    continue;
                }

                if let Some(cs) = self.constraints.get(m) {
                    for c in cs
                        .iter()
                        .filter(|c| !matches!(c, Constraint::Equal(_)))
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                    {
                        self.add_constraint(combined_meta, c.clone());
                    }
                    self.constraints.remove(m).unwrap();
                    merged.insert(*m);
                    // Record a new meta the first time that we use it; don't
                    // bother recording a new meta if we don't add any
                    // constraints. It should be safe to call this multiple times
                    new_metas.insert(combined_meta);
                }
                // Here, solved.get is equivalent to get_solution, because if
                // `m` had already been shunted, we wouldn't skipped it
                if let Some(solution) = self.solved.get(m) {
                    match self.solved.get(&combined_meta) {
                        Some(existing_solution) => {
                            if solution != existing_solution {
                                return Err(self.report_mismatch(
                                    *m,
                                    combined_meta,
                                    solution.clone(),
                                    existing_solution.clone(),
                                ));
                            }
                        }
                        None => {
                            self.solved.insert(combined_meta, solution.clone());
                        }
                    }
                }
                if self.variables.contains(m) {
                    self.variables.insert(combined_meta);
                    self.variables.remove(m);
                }
                self.shunted.insert(*m, combined_meta);
            }
        }
        Ok((new_metas, merged))
    }

    /// Inspect the constraints of a given metavariable and try to find a
    /// solution based on those.
    /// Returns whether a solution was found
    fn solve_meta(&mut self, meta: Meta) -> Result<bool, InferExtensionError> {
        let mut solved = false;
        for c in self.get_constraints(&meta).unwrap().clone().iter() {
            match c {
                // Just register the equality in the EqGraph, we'll process it later
                Constraint::Equal(other_meta) => {
                    self.eq_graph.register_eq(meta, *other_meta);
                }
                Constraint::Plus(r, other_meta) => {
                    if let Some(rs) = self.get_solution(other_meta) {
                        let mut rrs = rs.clone();
                        rrs.insert(r);
                        match self.get_solution(&meta) {
                            // Let's check that this is right?
                            Some(rs) => {
                                if rs != &rrs {
                                    return Err(self.report_mismatch(
                                        meta,
                                        *other_meta,
                                        rs.clone(),
                                        rrs,
                                    ));
                                }
                            }
                            None => {
                                self.add_solution(meta, rrs);
                                solved = true;
                            }
                        };
                    } else if let Some(superset) = self.get_solution(&meta) {
                        let subset = ExtensionSet::singleton(r).missing_from(superset);
                        self.add_solution(self.resolve(*other_meta), subset);
                        solved = true;
                    };
                }
            }
        }
        Ok(solved)
    }

    /// Tries to return concrete extensions for each node in the graph. This only
    /// works when there are no variables in the graph!
    ///
    /// What we really want is to give the concrete extensions where they're
    /// available. When there are variables, we should leave the graph as it is,
    /// but make sure that no matter what they're instantiated to, the graph
    /// still makes sense (should pass the extension validation check)
    pub fn results(&mut self) -> Result<ExtensionSolution, InferExtensionError> {
        // Check that all of the metavariables associated with nodes of the
        // graph are solved
        let mut results: ExtensionSolution = HashMap::new();
        for (loc, meta) in self.extensions.iter() {
            let rs = match self.get_solution(meta) {
                Some(rs) => Ok(rs.clone()),
                None => {
                    // If it depends on some other live meta, that's bad news.
                    // If it only depends on graph variables, then we don't have
                    // a *solution*, but it's fine
                    if self.live_var(meta).is_some() {
                        Err(InferExtensionError::Unsolved { location: *loc })
                    } else {
                        continue;
                    }
                }
            }?;
            results.insert(*loc, rs);
        }
        debug_assert!(self.live_metas().is_empty());
        Ok(results)
    }

    // Get the live var associated with a meta.
    // TODO: This should really be a list
    fn live_var(&self, m: &Meta) -> Option<Meta> {
        if self.variables.contains(m) || self.variables.contains(&self.resolve(*m)) {
            return None;
        }

        // TODO: We should be doing something to ensure that these are the same check...
        if self.get_solution(m).is_none() {
            if let Some(cs) = self.get_constraints(m) {
                for c in cs {
                    match c {
                        Constraint::Plus(_, m) => return self.live_var(m),
                        _ => panic!("we shouldn't be here!"),
                    }
                }
            }
            Some(*m)
        } else {
            None
        }
    }

    /// Return the set of "live" metavariables in the context.
    /// "Live" here means a metavariable:
    ///   - Is associated to a location in the graph in `UnifyContext.extensions`
    ///   - Is still unsolved
    ///   - Isn't a variable
    fn live_metas(&self) -> HashSet<Meta> {
        self.extensions
            .values()
            .filter_map(|m| self.live_var(m))
            .filter(|m| !self.variables.contains(m))
            .collect()
    }

    /// Iterates over a set of metas (the argument) and tries to solve
    /// them.
    /// Returns the metas that we solved
    fn solve_constraints(
        &mut self,
        vars: &HashSet<Meta>,
    ) -> Result<HashSet<Meta>, InferExtensionError> {
        let mut solved = HashSet::new();
        for m in vars.iter() {
            if self.solve_meta(*m)? {
                solved.insert(*m);
            }
        }
        Ok(solved)
    }

    /// Once the unification context is set up, attempt to infer ExtensionSets
    /// for all of the metavariables in the `UnificationContext`.
    ///
    /// Return a mapping from locations in the graph to concrete `ExtensionSets`
    /// where it was possible to infer them. If it wasn't possible to infer a
    /// *concrete* `ExtensionSet`, e.g. if the ExtensionSet relies on an open
    /// variable in the toplevel graph, don't include that location in the map
    pub fn main_loop(&mut self) -> Result<ExtensionSolution, InferExtensionError> {
        let mut remaining = HashSet::<Meta>::from_iter(self.constraints.keys().cloned());

        // Keep going as long as we're making progress (= merging and solving nodes)
        loop {
            // Try to solve metas with the information we have now. This may
            // register new equalities on the EqGraph
            let to_delete = self.solve_constraints(&remaining)?;
            // Merge metas based on the equalities we just registered
            let (new, merged) = self.merge_equal_metas()?;
            // All of the metas for which we've made progress
            let delta: HashSet<Meta> = HashSet::from_iter(to_delete.union(&merged).cloned());

            // Clean up dangling constraints on solved metavariables
            to_delete.iter().for_each(|m| {
                self.constraints.remove(m);
            });
            // Remove solved and merged metas from remaining "to solve" list
            delta.iter().for_each(|m| {
                remaining.remove(m);
            });

            // If we made no progress, we're done!
            if delta.is_empty() && new.is_empty() {
                break;
            }
            remaining.extend(new)
        }
        self.results()
    }

    /// Instantiate all variables in the graph with the empty extension set.
    /// This is done to solve metas which depend on variables, which allows
    /// us to come up with a fully concrete solution to pass into validation.
    pub fn instantiate_variables(&mut self) {
        for m in self.variables.clone().into_iter() {
            if !self.solved.contains_key(&m) {
                self.add_solution(m, ExtensionSet::new());
            }
        }
        self.variables = HashSet::new();
    }
}

#[cfg(test)]
mod test {
    use std::error::Error;

    use super::*;
    use crate::builder::{BuildError, DFGBuilder, Dataflow, DataflowHugr};
    use crate::extension::{ExtensionSet, EMPTY_REG};
    use crate::hugr::HugrMut;
    use crate::hugr::{validate::ValidationError, Hugr, HugrView, NodeType};
    use crate::ops::{self, dataflow::IOTrait, handle::NodeHandle};
    use crate::type_row;
    use crate::types::{FunctionType, Type};

    use cool_asserts::assert_matches;
    use portgraph::NodeIndex;

    const BIT: Type = crate::extension::prelude::USIZE_T;

    #[test]
    // Build up a graph with some holes in its extension requirements, and infer
    // them.
    fn from_graph() -> Result<(), Box<dyn Error>> {
        let rs = ExtensionSet::from_iter(["A".into(), "B".into(), "C".into()]);
        let main_sig =
            FunctionType::new(type_row![BIT, BIT], type_row![BIT]).with_extension_delta(&rs);

        let op = ops::DFG {
            signature: main_sig,
        };

        let root_node = NodeType::open_extensions(op);
        let mut hugr = Hugr::new(root_node);

        let input = NodeType::open_extensions(ops::Input::new(type_row![BIT, BIT]));
        let output = NodeType::open_extensions(ops::Output::new(type_row![BIT]));

        let input = hugr.add_node_with_parent(hugr.root(), input)?;
        let output = hugr.add_node_with_parent(hugr.root(), output)?;

        assert_matches!(hugr.get_io(hugr.root()), Some(_));

        let add_a_sig = FunctionType::new(type_row![BIT], type_row![BIT])
            .with_extension_delta(&ExtensionSet::singleton(&"A".into()));

        let add_b_sig = FunctionType::new(type_row![BIT], type_row![BIT])
            .with_extension_delta(&ExtensionSet::singleton(&"B".into()));

        let add_ab_sig = FunctionType::new(type_row![BIT], type_row![BIT])
            .with_extension_delta(&ExtensionSet::from_iter(["A".into(), "B".into()]));

        let mult_c_sig = FunctionType::new(type_row![BIT, BIT], type_row![BIT])
            .with_extension_delta(&ExtensionSet::singleton(&"C".into()));

        let add_a = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::DFG {
                signature: add_a_sig,
            }),
        )?;
        let add_b = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::DFG {
                signature: add_b_sig,
            }),
        )?;
        let add_ab = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::DFG {
                signature: add_ab_sig,
            }),
        )?;
        let mult_c = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::DFG {
                signature: mult_c_sig,
            }),
        )?;

        hugr.connect(input, 0, add_a, 0)?;
        hugr.connect(add_a, 0, add_b, 0)?;
        hugr.connect(add_b, 0, mult_c, 0)?;

        hugr.connect(input, 1, add_ab, 0)?;
        hugr.connect(add_ab, 0, mult_c, 1)?;

        hugr.connect(mult_c, 0, output, 0)?;

        let (_, closure) = infer_extensions(&hugr)?;
        let empty = ExtensionSet::new();
        let ab = ExtensionSet::from_iter(["A".into(), "B".into()]);
        let abc = ExtensionSet::from_iter(["A".into(), "B".into(), "C".into()]);
        assert_eq!(
            *closure.get(&(hugr.root(), Direction::Incoming)).unwrap(),
            empty
        );
        assert_eq!(
            *closure.get(&(hugr.root(), Direction::Outgoing)).unwrap(),
            abc
        );
        assert_eq!(*closure.get(&(mult_c, Direction::Incoming)).unwrap(), ab);
        assert_eq!(*closure.get(&(mult_c, Direction::Outgoing)).unwrap(), abc);
        assert_eq!(*closure.get(&(add_ab, Direction::Incoming)).unwrap(), empty);
        assert_eq!(*closure.get(&(add_ab, Direction::Outgoing)).unwrap(), ab);
        assert_eq!(*closure.get(&(add_ab, Direction::Incoming)).unwrap(), empty);
        assert_eq!(
            *closure.get(&(add_b, Direction::Incoming)).unwrap(),
            ExtensionSet::singleton(&"A".into())
        );
        Ok(())
    }

    #[test]
    // Basic test that the `Plus` constraint works
    fn plus() -> Result<(), InferExtensionError> {
        let hugr = Hugr::default();
        let mut ctx = UnificationContext::new(&hugr);

        let metas: Vec<Meta> = (2..8)
            .map(|i| {
                let meta = ctx.fresh_meta();
                ctx.extensions
                    .insert((NodeIndex::new(i).into(), Direction::Incoming), meta);
                meta
            })
            .collect();

        ctx.solved
            .insert(metas[2], ExtensionSet::singleton(&"A".into()));
        ctx.add_constraint(metas[1], Constraint::Equal(metas[2]));
        ctx.add_constraint(metas[0], Constraint::Plus("B".into(), metas[2]));
        ctx.add_constraint(metas[4], Constraint::Plus("C".into(), metas[0]));
        ctx.add_constraint(metas[3], Constraint::Equal(metas[4]));
        ctx.add_constraint(metas[5], Constraint::Equal(metas[0]));
        ctx.main_loop()?;

        let a = ExtensionSet::singleton(&"A".into());
        let mut ab = a.clone();
        ab.insert(&"B".into());
        let mut abc = ab.clone();
        abc.insert(&"C".into());

        assert_eq!(ctx.get_solution(&metas[0]).unwrap(), &ab);
        assert_eq!(ctx.get_solution(&metas[1]).unwrap(), &a);
        assert_eq!(ctx.get_solution(&metas[2]).unwrap(), &a);
        assert_eq!(ctx.get_solution(&metas[3]).unwrap(), &abc);
        assert_eq!(ctx.get_solution(&metas[4]).unwrap(), &abc);
        assert_eq!(ctx.get_solution(&metas[5]).unwrap(), &ab);

        Ok(())
    }

    #[test]
    // This generates a solution that causes validation to fail
    // because of a missing lift node
    fn missing_lift_node() -> Result<(), Box<dyn Error>> {
        let builder = DFGBuilder::new(
            FunctionType::new(type_row![BIT], type_row![BIT])
                .with_extension_delta(&ExtensionSet::singleton(&"R".into())),
        )?;
        let [w] = builder.input_wires_arr();
        let hugr = builder.finish_hugr_with_outputs([w], &EMPTY_REG);

        // Fail to catch the actual error because it's a difference between I/O
        // nodes and their parents and `report_mismatch` isn't yet smart enough
        // to handle that.
        assert_matches!(
            hugr,
            Err(BuildError::InvalidHUGR(ValidationError::CantInfer(_)))
        );
        Ok(())
    }

    #[test]
    // Tests that we can succeed even when all variables don't have concrete
    // extension sets, and we have an open variable at the start of the graph.
    fn open_variables() -> Result<(), InferExtensionError> {
        let mut ctx = UnificationContext::new(&Hugr::default());
        let a = ctx.fresh_meta();
        let b = ctx.fresh_meta();
        let ab = ctx.fresh_meta();
        // Some nonsense so that the constraints register as "live"
        ctx.extensions
            .insert((NodeIndex::new(2).into(), Direction::Outgoing), a);
        ctx.extensions
            .insert((NodeIndex::new(3).into(), Direction::Outgoing), b);
        ctx.extensions
            .insert((NodeIndex::new(4).into(), Direction::Incoming), ab);
        ctx.variables.insert(a);
        ctx.variables.insert(b);
        ctx.add_constraint(ab, Constraint::Plus("A".into(), b));
        ctx.add_constraint(ab, Constraint::Plus("B".into(), a));
        let solution = ctx.main_loop()?;
        // We'll only find concrete solutions for the Incoming/Outgoing sides of
        // the main node created by `Hugr::default`
        assert_eq!(solution.len(), 2);
        Ok(())
    }

    #[test]
    // Infer the extensions on a child node with no inputs
    fn dangling_src() -> Result<(), Box<dyn Error>> {
        let rs = ExtensionSet::singleton(&"R".into());
        let root_signature =
            FunctionType::new(type_row![BIT], type_row![BIT]).with_extension_delta(&rs);
        let mut builder = DFGBuilder::new(root_signature)?;
        let [input_wire] = builder.input_wires_arr();

        let add_r_sig = FunctionType::new(type_row![BIT], type_row![BIT]).with_extension_delta(&rs);

        let add_r = builder.add_dataflow_node(
            NodeType::open_extensions(ops::DFG {
                signature: add_r_sig,
            }),
            [input_wire],
        )?;
        let [wl] = add_r.outputs_arr();

        // Dangling thingy
        let src_sig = FunctionType::new(type_row![], type_row![BIT])
            .with_extension_delta(&ExtensionSet::new());
        let src = builder.add_dataflow_node(
            NodeType::open_extensions(ops::DFG { signature: src_sig }),
            [],
        )?;
        let [wr] = src.outputs_arr();

        let mult_sig = FunctionType::new(type_row![BIT, BIT], type_row![BIT])
            .with_extension_delta(&ExtensionSet::new());
        // Mult has open extension requirements, which we should solve to be "R"
        let mult = builder.add_dataflow_node(
            NodeType::open_extensions(ops::DFG {
                signature: mult_sig,
            }),
            [wl, wr],
        )?;
        let [w] = mult.outputs_arr();

        builder.set_outputs([w])?;
        let mut hugr = builder.base;
        let closure = hugr.infer_extensions()?;
        assert!(closure.is_empty());
        assert_eq!(
            hugr.get_nodetype(src.node())
                .signature()
                .unwrap()
                .output_extensions(),
            rs
        );
        assert_eq!(
            hugr.get_nodetype(mult.node())
                .signature()
                .unwrap()
                .input_extensions,
            rs
        );
        assert_eq!(
            hugr.get_nodetype(mult.node())
                .signature()
                .unwrap()
                .output_extensions(),
            rs
        );
        Ok(())
    }

    #[test]
    fn resolve_test() -> Result<(), InferExtensionError> {
        let mut ctx = UnificationContext::new(&Hugr::default());
        let m0 = ctx.fresh_meta();
        let m1 = ctx.fresh_meta();
        let m2 = ctx.fresh_meta();
        ctx.add_constraint(m0, Constraint::Equal(m1));
        ctx.main_loop()?;
        let mid0 = ctx.resolve(m0);
        assert_eq!(ctx.resolve(m0), ctx.resolve(m1));
        ctx.add_constraint(mid0, Constraint::Equal(m2));
        ctx.main_loop()?;
        assert_eq!(ctx.resolve(m0), ctx.resolve(m2));
        assert_eq!(ctx.resolve(m1), ctx.resolve(m2));
        assert!(ctx.resolve(m0) != mid0);
        Ok(())
    }

    #[test]
    fn minus_test() -> Result<(), Box<dyn Error>> {
        let const_true = ops::Const::true_val();
        const BOOLEAN: Type = Type::new_simple_predicate(2);
        let just_bool = type_row![BOOLEAN];

        let abc = ExtensionSet::from_iter(["A".into(), "B".into(), "C".into()]);

        // Parent graph is closed
        let mut hugr = Hugr::new(NodeType::pure(ops::DFG {
            signature: FunctionType::new(type_row![], just_bool.clone()).with_extension_delta(&abc),
        }));

        let _input = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::Input { types: type_row![] }),
        )?;
        let output = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::Output {
                types: just_bool.clone(),
            }),
        )?;

        let child = hugr.add_node_with_parent(
            hugr.root(),
            NodeType::open_extensions(ops::DFG {
                signature: FunctionType::new(type_row![], just_bool.clone())
                    .with_extension_delta(&abc),
            }),
        )?;
        let _ichild = hugr.add_node_with_parent(
            child,
            NodeType::open_extensions(ops::Input { types: type_row![] }),
        )?;
        let ochild = hugr.add_node_with_parent(
            child,
            NodeType::open_extensions(ops::Output {
                types: just_bool.clone(),
            }),
        )?;

        let const_node = hugr.add_node_with_parent(child, NodeType::open_extensions(const_true))?;
        let lift_node = hugr.add_node_with_parent(
            child,
            NodeType::open_extensions(ops::LeafOp::Lift {
                type_row: just_bool,
                new_extension: "C".into(),
            }),
        )?;

        hugr.connect(const_node, 0, lift_node, 0)?;
        hugr.connect(lift_node, 0, ochild, 0)?;
        hugr.connect(child, 0, output, 0)?;

        let (sol, _) = infer_extensions(&hugr)?;

        // The solution for the const node should be {A, B}!
        assert_eq!(
            *sol.get(&(const_node, Direction::Outgoing)).unwrap(),
            ExtensionSet::from_iter(["A".into(), "B".into()])
        );

        Ok(())
    }
}