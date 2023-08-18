//! Validation routines for instantiations of a extension ops and types in a
//! Hugr.

use std::collections::HashMap;

use thiserror::Error;

use crate::hugr::NodeType;
use crate::{Direction, Hugr, HugrView, Node, Port};

use super::ExtensionSet;

/// Context for validating the extension requirements defined in a Hugr.
#[derive(Debug, Clone, Default)]
pub struct ExtensionValidator {
    /// Extension requirements associated with each edge
    extensions: HashMap<(Node, Direction), ExtensionSet>,
}

impl ExtensionValidator {
    /// Initialise a new extension validator, pre-computing the extension
    /// requirements for each node in the Hugr.
    pub fn new(hugr: &Hugr) -> Self {
        let mut validator = ExtensionValidator {
            extensions: HashMap::new(),
        };

        for node in hugr.nodes() {
            validator.gather_extensions(&node, hugr.get_nodetype(node));
        }

        validator
    }

    /// Use the signature supplied by a dataflow node to work out the
    /// extension requirements for all of its input and output edges, then put
    /// those requirements in the extension validation context.
    fn gather_extensions(&mut self, node: &Node, node_type: &NodeType) {
        if let Some(sig) = node_type.signature() {
            for dir in Direction::BOTH {
                assert!(self
                    .extensions
                    .insert((*node, dir), sig.get_extension(&dir))
                    .is_none());
            }
        }
    }

    /// Get the input or output extension requirements for a particular node in the Hugr.
    ///
    /// # Errors
    ///
    /// If the node extensions are missing.
    fn query_extensions(
        &self,
        node: Node,
        dir: Direction,
    ) -> Result<&ExtensionSet, ExtensionError> {
        self.extensions
            .get(&(node, dir))
            .ok_or(ExtensionError::MissingInputExtensions(node))
    }

    /// Check that two `PortIndex` have compatible extension requirements,
    /// according to the information accumulated by `gather_extensions`.
    ///
    /// This extension checking assumes that free extension variables
    ///   (e.g. implicit lifting of `A -> B` to `[R]A -> [R]B`)
    /// and adding of lift nodes
    ///   (i.e. those which transform an edge from `A` to `[R]A`)
    /// has already been done.
    pub fn check_extensions_compatible(
        &self,
        src: &(Node, Port),
        tgt: &(Node, Port),
    ) -> Result<(), ExtensionError> {
        let rs_src = self.query_extensions(src.0, Direction::Outgoing)?;
        let rs_tgt = self.query_extensions(tgt.0, Direction::Incoming)?;

        if rs_src == rs_tgt {
            Ok(())
        } else if rs_src.is_subset(rs_tgt) {
            // The extra extension requirements reside in the target node.
            // If so, we can fix this mismatch with a lift node
            Err(ExtensionError::TgtExceedsSrcExtensions {
                from: src.0,
                from_offset: src.1,
                from_extensions: rs_src.clone(),
                to: tgt.0,
                to_offset: tgt.1,
                to_extensions: rs_tgt.clone(),
            })
        } else {
            Err(ExtensionError::SrcExceedsTgtExtensions {
                from: src.0,
                from_offset: src.1,
                from_extensions: rs_src.clone(),
                to: tgt.0,
                to_offset: tgt.1,
                to_extensions: rs_tgt.clone(),
            })
        }
    }

    /// Check that a pair of input and output nodes declare the same extensions
    /// as in the signature of their parents.
    pub fn validate_io_extensions(
        &self,
        parent: Node,
        input: Node,
        output: Node,
    ) -> Result<(), ExtensionError> {
        let parent_input_extensions = self.query_extensions(parent, Direction::Incoming)?;
        let parent_output_extensions = self.query_extensions(parent, Direction::Outgoing)?;
        for dir in Direction::BOTH {
            let input_extensions = self.query_extensions(input, dir)?;
            let output_extensions = self.query_extensions(output, dir)?;
            if parent_input_extensions != input_extensions {
                return Err(ExtensionError::ParentIOExtensionMismatch {
                    parent,
                    parent_extensions: parent_input_extensions.clone(),
                    child: input,
                    child_extensions: input_extensions.clone(),
                });
            };
            if parent_output_extensions != output_extensions {
                return Err(ExtensionError::ParentIOExtensionMismatch {
                    parent,
                    parent_extensions: parent_output_extensions.clone(),
                    child: output,
                    child_extensions: output_extensions.clone(),
                });
            };
        }
        Ok(())
    }
}

/// Errors that can occur while validating a Hugr.
#[derive(Debug, Clone, PartialEq, Error)]
#[allow(missing_docs)]
pub enum ExtensionError {
    /// Missing lift node
    #[error("Extensions at target node {to:?} ({to_offset:?}) ({to_extensions}) exceed those at source {from:?} ({from_offset:?}) ({from_extensions})")]
    TgtExceedsSrcExtensions {
        from: Node,
        from_offset: Port,
        from_extensions: ExtensionSet,
        to: Node,
        to_offset: Port,
        to_extensions: ExtensionSet,
    },
    /// Too many extension requirements coming from src
    #[error("Extensions at source node {from:?} ({from_offset:?}) ({from_extensions}) exceed those at target {to:?} ({to_offset:?}) ({to_extensions})")]
    SrcExceedsTgtExtensions {
        from: Node,
        from_offset: Port,
        from_extensions: ExtensionSet,
        to: Node,
        to_offset: Port,
        to_extensions: ExtensionSet,
    },
    #[error("Missing input extensions for node {0:?}")]
    MissingInputExtensions(Node),
    #[error("Extensions of I/O node ({child:?}) {child_extensions:?} don't match those expected by parent node ({parent:?}): {parent_extensions:?}")]
    ParentIOExtensionMismatch {
        parent: Node,
        parent_extensions: ExtensionSet,
        child: Node,
        child_extensions: ExtensionSet,
    },
}