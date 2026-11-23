//! `?` parameter markers (port of src/parser/param_bind.cpp).

use super::ast::{CommandNode, ParsedStatement, UpdateSetElement};
use crate::error::{fail, Errc, Result};
use crate::value::Value;

fn collect_set_operands<'a>(set: &'a mut [UpdateSetElement], out: &mut Vec<&'a mut Value>) {
    for element in set.iter_mut() {
        if let Some(v) = element.rhs_value.as_mut() {
            v.collect_param_slots(out);
        }
        if let Some(v) = element.if_not_exists_value.as_mut() {
            v.collect_param_slots(out);
        }
    }
}

/// Every unbound `?` slot in the command, in ordinal (source) order.
pub fn collect_param_slots<'a>(command: &'a mut CommandNode, out: &mut Vec<&'a mut Value>) {
    match command {
        CommandNode::Insert(cmd) => {
            for row in cmd.rows.iter_mut() {
                for value in row.iter_mut() {
                    value.collect_param_slots(out);
                }
            }
        }
        CommandNode::Select(cmd) => {
            if let Some(w) = cmd.where_.as_mut() {
                w.collect_param_slots(out);
            }
            if let Some(v) = cmd.search_vector.as_mut() {
                v.collect_param_slots(out);
            }
        }
        CommandNode::Update(cmd) => {
            collect_set_operands(&mut cmd.set, out);
            if let Some(w) = cmd.where_.as_mut() {
                w.collect_param_slots(out);
            }
        }
        CommandNode::Delete(cmd) => {
            if let Some(w) = cmd.where_.as_mut() {
                w.collect_param_slots(out);
            }
        }
        CommandNode::Transact(cmd) => {
            for item in cmd.items.iter_mut() {
                collect_param_slots(item, out);
            }
        }
        _ => {}
    }
}

fn count_node(command: &CommandNode) -> usize {
    let set_count = |set: &[UpdateSetElement]| -> usize {
        set.iter()
            .map(|e| {
                e.rhs_value.as_ref().map_or(0, Value::count_params)
                    + e.if_not_exists_value
                        .as_ref()
                        .map_or(0, Value::count_params)
            })
            .sum()
    };
    match command {
        CommandNode::Insert(cmd) => cmd.rows.iter().flatten().map(Value::count_params).sum(),
        CommandNode::Select(cmd) => {
            cmd.where_.as_ref().map_or(0, |w| w.count_params())
                + cmd.search_vector.as_ref().map_or(0, Value::count_params)
        }
        CommandNode::Update(cmd) => {
            set_count(&cmd.set) + cmd.where_.as_ref().map_or(0, |w| w.count_params())
        }
        CommandNode::Delete(cmd) => cmd.where_.as_ref().map_or(0, |w| w.count_params()),
        CommandNode::Transact(cmd) => cmd.items.iter().map(count_node).sum(),
        _ => 0,
    }
}

/// Number of unbound `?` markers in the statement.
pub fn count_param_markers(statement: &ParsedStatement) -> usize {
    count_node(&statement.command)
}

/// Bind `values` positionally to the statement's `?` markers.
pub fn bind_params(statement: &mut ParsedStatement, values: &[Value]) -> Result<()> {
    let mut slots = Vec::new();
    collect_param_slots(&mut statement.command, &mut slots);
    if slots.len() != values.len() {
        return fail(
            Errc::InvalidArgument,
            format!(
                "statement has {} parameter marker(s) but {} value(s) were bound",
                slots.len(),
                values.len()
            ),
        );
    }
    for (slot, value) in slots.into_iter().zip(values) {
        *slot = value.clone();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_one;

    #[test]
    fn count_and_bind() {
        let mut s =
            parse_one("update t set a = ?, b = if_not_exists(b, ?) where k = ? and j in (?, 1)")
                .unwrap();
        assert_eq!(count_param_markers(&s), 4);
        let vals: Vec<Value> = (1..=4).map(|i| Value::N(i.to_string())).collect();
        bind_params(&mut s, &vals).unwrap();
        assert_eq!(count_param_markers(&s), 0);
        let mut s = parse_one("insert into t (a) values (?)").unwrap();
        assert!(bind_params(&mut s, &[]).is_err());
    }
}
