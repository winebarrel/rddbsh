//! Expression engine: symbol table, attribute serialization, and the WHERE tree.
pub mod attribute_serialize;
pub mod logical;
pub mod symbol_table;
pub mod where_clause;

pub use attribute_serialize::serialize_attribute;
pub use logical::{KeyMap, KeySchemaView, LogicalExpr, LogicalKind};
pub use symbol_table::SymbolTable;
pub use where_clause::{QueryKeyPlan, Where};

const PREFIX_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const PREFIX_LENGTH: usize = 3;

/// Deterministic 3-character alias prefix for the given counter value
/// (port of expressions/prefix.hpp): `aaa`, `baa`, `caa`, ...
pub fn unique_prefix(counter: u64) -> String {
    let base = PREFIX_CHARS.len() as u64;
    let mut value = counter;
    let mut prefix = String::with_capacity(PREFIX_LENGTH);
    for _ in 0..PREFIX_LENGTH {
        prefix.push(PREFIX_CHARS[(value % base) as usize] as char);
        value /= base;
    }
    prefix
}
