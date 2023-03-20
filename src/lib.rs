//#![warn(missing_docs)] TODO: Re-enable this once the library is more mature

//! `hugr` is the Hierarchical Unified Graph Representation of quantum circuits
//! and operations in the Quantinuum ecosystem.
//!
//! # Features
//!
//! - `serde` enables serialization and deserialization of the components and
//!   structures.
//!

pub mod hugr;
pub mod macros;
pub mod ops;
pub mod types;

pub use hugr::Hugr;

#[cfg(test)]
mod test {
    // Example test
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
