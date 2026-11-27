// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! A not-equal on a sort key (ddbsh 0.12.9): `<>`, `!=` and `NOT x = v` are
//! one operator, and with the partition key `=` and a single not-equal on the
//! (last) sort key the read is two Queries, `sk < v` then `sk > v`.
//! Ports of the not-equal cases in expression_test, planner_test,
//! explain_exec_test, transact_exec_test and timely_output_test.

use super::delete_executor::execute_delete;
use super::select_cursor::open_select_cursor;
use super::select_executor::execute_select_explain;
use super::test_support::{table, FakeDdbClient};
use super::transact_executor::execute_transact;
use super::update_executor::execute_update;
use crate::client::*;
use crate::expressions::{
    unique_prefix, KeySchemaView, KeyTypes, LogicalExpr, NeLeg, SymbolTable, Where,
};
use crate::outcome::*;
use crate::parser::ast::*;
use crate::parser::grammar::parse_one;
use crate::planner::{plan_select, SelectApi, SelectPlan};
use crate::table_schema::{AttributeDef, ScalarType};
use crate::value::Value;

fn num(s: &str) -> Value {
    Value::N(s.into())
}
fn str_(s: &str) -> Value {
    Value::S(s.into())
}
fn cmp(lhs: &str, op: &str, v: Value) -> LogicalExpr {
    LogicalExpr::comparison(lhs, op, v)
}
fn and(a: LogicalExpr, b: LogicalExpr) -> LogicalExpr {
    LogicalExpr::make_and(a, b)
}
fn or(a: LogicalExpr, b: LogicalExpr) -> LogicalExpr {
    LogicalExpr::make_or(a, b)
}
fn negated(mut e: LogicalExpr) -> LogicalExpr {
    e.negate();
    e
}
fn v(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}
fn gen() -> impl FnMut() -> String {
    let mut c = 0u64;
    move || {
        let p = unique_prefix(c);
        c += 1;
        p
    }
}
fn row(pairs: &[(&str, Value)]) -> ItemRow {
    ItemRow {
        attributes: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

// ---- Where / LogicalExpr ---------------------------------------------------

#[test]
fn range_not_equal_on_key_splits_into_two_queries() {
    for op in ["!=", "<>"] {
        let w = Where::new(and(cmp("pk", "=", num("1")), cmp("rk", op, num("5"))));
        assert!(!w.can_getitem(&v(&["pk"]), &v(&["rk"])));
        let plan = w.plan_query(&v(&["pk"]), &v(&["rk"]), None).unwrap();
        assert_eq!(plan.ne_split.as_deref(), Some("rk"), "{op}");
        let mut lower = SymbolTable::new("aaa");
        assert_eq!(
            w.query_key_condition_expression(&plan, Some(&mut lower), NeLeg::Lower),
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2"
        );
        let mut upper = SymbolTable::new("aaa");
        assert_eq!(
            w.query_key_condition_expression(&plan, Some(&mut upper), NeLeg::Upper),
            "#aaaa1 = :vaaa1 AND #aaaa2 > :vaaa2"
        );
        let mut filter = SymbolTable::new("aaa");
        assert_eq!(w.query_filter_expression(&plan, Some(&mut filter)), "");
    }
}

#[test]
fn meaning_is_independent_of_spelling() {
    let eq_op = cmp("a", "=", num("1"));
    let bang = cmp("a", "!=", num("1"));
    let ltgt = cmp("a", "<>", num("1"));
    let not_ltgt = negated(cmp("a", "<>", num("1")));
    let negated_eq = negated(cmp("a", "=", num("1")));
    assert!(eq_op.means_equal());
    assert!(!eq_op.means_not_equal());
    assert!(bang.means_not_equal());
    assert!(ltgt.means_not_equal());
    assert!(negated_eq.means_not_equal());
    assert!(not_ltgt.means_equal());
    assert!(!not_ltgt.means_not_equal());
    let lt = cmp("a", "<", num("1"));
    assert!(!lt.means_equal());
    assert!(!lt.means_not_equal());
    assert_eq!(not_ltgt.count_gud_key("a"), 1);
    assert_eq!(ltgt.count_gud_key("a"), 0);
    assert_eq!(bang.count_gud_key("a"), 0);
}

#[test]
fn filters_keep_the_typed_spelling() {
    let mut st1 = SymbolTable::new("aaa");
    let bang = Where::new(cmp("x", "!=", num("3")));
    assert_eq!(
        bang.scan_filter_expression(Some(&mut st1)),
        "NOT (#aaaa1 = :vaaa1)"
    );
    let mut st2 = SymbolTable::new("aaa");
    let ltgt = Where::new(cmp("x", "<>", num("3")));
    assert_eq!(
        ltgt.scan_filter_expression(Some(&mut st2)),
        "#aaaa1 <> :vaaa1"
    );
}

#[test]
fn ltgt_on_sort_key_is_not_a_key_condition_unsplit() {
    let tree = and(cmp("pk", "=", num("1")), cmp("rk", "<>", num("5")));
    let (pk, rk) = (v(&["pk"]), v(&["rk"]));
    assert!(!tree.query_safe(KeySchemaView::new(&pk, &rk)));
    let view = KeySchemaView {
        pk: &pk,
        sk: &rk,
        ne_split: Some("rk"),
        leg: NeLeg::Lower,
    };
    assert!(tree.query_safe(view));
}

#[test]
fn not_ne_on_sort_key_is_getitem() {
    for op in ["!=", "<>"] {
        let w = Where::new(and(
            cmp("pk", "=", num("1")),
            negated(cmp("rk", op, num("5"))),
        ));
        assert!(w.can_getitem(&v(&["pk"]), &v(&["rk"])), "{op}");
        assert_eq!(w.get_gud_key(&v(&["pk"]), &v(&["rk"])).len(), 2);
    }
}

#[test]
fn not_ltgt_renders_as_equality_in_a_key_condition() {
    let w = Where::new(and(
        cmp("pk", "=", num("1")),
        negated(cmp("rk", "<>", num("5"))),
    ));
    let plan = w.plan_query(&v(&["pk"]), &v(&["rk"]), None).unwrap();
    assert!(plan.ne_split.is_none());
    let mut st = SymbolTable::new("aaa");
    assert_eq!(
        w.query_key_condition_expression(&plan, Some(&mut st), NeLeg::Lower),
        "#aaaa1 = :vaaa1 AND #aaaa2 = :vaaa2"
    );
}

#[test]
fn ne_key_value_only_for_a_top_level_conjunct() {
    let plain = and(cmp("pk", "=", num("1")), cmp("rk", "<>", num("5")));
    assert_eq!(plain.ne_key_value("rk"), Some(&num("5")));
    assert_eq!(plain.ne_key_value("pk"), None);
    let under_or = and(
        cmp("pk", "=", num("1")),
        or(cmp("rk", "<>", num("5")), cmp("x", "=", num("1"))),
    );
    assert_eq!(under_or.ne_key_value("rk"), None);
    let neg_and = negated(and(cmp("pk", "=", num("1")), cmp("rk", "!=", num("5"))));
    assert_eq!(neg_and.ne_key_value("rk"), None);
    let not_ne = negated(cmp("rk", "<>", num("5")));
    assert_eq!(not_ne.ne_key_value("rk"), None);
}

#[test]
fn no_split_when_sort_key_referenced_twice_under_or_negated_and_or_on_pk() {
    let (pk, rk) = (v(&["pk"]), v(&["rk"]));
    let twice = Where::new(and(
        and(cmp("pk", "=", num("1")), cmp("rk", "<>", num("5"))),
        cmp("rk", ">", num("2")),
    ));
    assert!(twice.plan_query(&pk, &rk, None).is_none());
    let under_or = Where::new(and(
        cmp("pk", "=", num("1")),
        or(cmp("rk", "<>", num("5")), cmp("x", "=", num("1"))),
    ));
    assert!(under_or.plan_query(&pk, &rk, None).is_none());
    let neg = Where::new(negated(and(
        cmp("pk", "=", num("1")),
        cmp("rk", "<>", num("5")),
    )));
    assert!(neg.plan_query(&pk, &rk, None).is_none());
    let on_pk = Where::new(and(cmp("pk", "<>", num("1")), cmp("rk", "=", num("5"))));
    assert!(on_pk.plan_query(&pk, &rk, None).is_none());
}

#[test]
fn type_rule() {
    let (pk, rk) = (v(&["pk"]), v(&["rk"]));
    let number_rk: KeyTypes = [
        ("pk".to_string(), ScalarType::N),
        ("rk".to_string(), ScalarType::N),
    ]
    .into_iter()
    .collect();
    let w = Where::new(and(cmp("pk", "=", num("1")), cmp("rk", "<>", str_("x"))));
    assert!(w.plan_query(&pk, &rk, Some(&number_rk)).is_none());
    // With no declared type, any S/N/B literal splits.
    assert!(w.plan_query(&pk, &rk, None).is_some());
    let w = Where::new(and(cmp("pk", "=", num("1")), cmp("rk", "<>", num("7"))));
    let plan = w.plan_query(&pk, &rk, Some(&number_rk)).unwrap();
    assert!(plan.ne_split.is_some());
    for val in [Value::Null, Value::Bool(true), Value::Param(1)] {
        let w = Where::new(and(cmp("pk", "=", num("1")), cmp("rk", "!=", val)));
        assert!(w.plan_query(&pk, &rk, None).is_none());
    }
}

#[test]
fn composite_sort_key_splits_only_on_the_last_component() {
    let (g, hr) = (v(&["g"]), v(&["h", "r"]));
    let w = Where::new(and(cmp("g", "=", str_("a")), cmp("h", "<>", num("5"))));
    assert_eq!(
        w.plan_query(&g, &hr, None).unwrap().ne_split.as_deref(),
        Some("h")
    );
    let w = Where::new(and(
        and(cmp("g", "=", str_("a")), cmp("h", "=", num("5"))),
        cmp("r", "<>", num("5")),
    ));
    let plan = w.plan_query(&g, &hr, None).unwrap();
    assert_eq!(plan.ne_split.as_deref(), Some("r"));
    let mut st = SymbolTable::new("aaa");
    assert_eq!(
        w.query_key_condition_expression(&plan, Some(&mut st), NeLeg::Upper),
        "#aaaa1 = :vaaa1 AND #aaaa2 = :vaaa2 AND #aaaa3 > :vaaa3"
    );
    let w = Where::new(and(
        and(cmp("g", "=", str_("a")), cmp("h", "<>", num("5"))),
        cmp("r", "=", num("5")),
    ));
    assert!(w.plan_query(&g, &hr, None).is_none());
}

#[test]
fn split_keeps_non_key_predicates_in_the_filter() {
    let w = Where::new(and(
        and(cmp("pk", "=", num("1")), cmp("rk", "<>", num("5"))),
        cmp("x", "<>", num("3")),
    ));
    let plan = w.plan_query(&v(&["pk"]), &v(&["rk"]), None).unwrap();
    let mut st = SymbolTable::new("aaa");
    assert_eq!(
        w.query_key_condition_expression(&plan, Some(&mut st), NeLeg::Lower),
        "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2"
    );
    assert_eq!(
        w.query_filter_expression(&plan, Some(&mut st)),
        "#aaaa3 <> :vaaa3"
    );
}

#[test]
fn condition_check_keeps_a_not_equal_key_leaf_in_both_spellings() {
    let base = || and(cmp("pk", "=", num("1")), cmp("rk", "=", num("2")));
    let mut st = SymbolTable::new("aaa");
    let w = Where::new(and(base(), cmp("rk", "<>", num("2"))));
    assert_eq!(
        w.update_delete_condition_check("pk", "rk", Some(&mut st)),
        "#aaaa1 <> :vaaa1"
    );
    let mut st = SymbolTable::new("aaa");
    let w = Where::new(and(base(), cmp("rk", "!=", num("2"))));
    assert_eq!(
        w.update_delete_condition_check("pk", "rk", Some(&mut st)),
        "NOT (#aaaa1 = :vaaa1)"
    );
    // NOT rk <> 2 means rk = 2: a plain key comparison, dropped.
    let mut st = SymbolTable::new("aaa");
    let w = Where::new(and(
        cmp("pk", "=", num("1")),
        negated(cmp("rk", "<>", num("2"))),
    ));
    assert_eq!(
        w.update_delete_condition_check("pk", "rk", Some(&mut st)),
        ""
    );
}

// ---- planner -----------------------------------------------------------------

/// ddbsh_ne: (pk N HASH, sk N RANGE), LSI ne_lsi (pk, l), GSI ne_gsi (g, h),
/// composite GSI ne_cgsi ((g, q) HASH, (h, r) RANGE); declared types as the
/// real table reports them (the `ine-*` golden family's seed).
fn ne_table() -> DescribeTableOut {
    let mut t = table("ddbsh_ne", "pk", "sk");
    let idx = |name: &str, hash: &[&str], range: &[&str]| SecondaryIndexSchema {
        index_name: name.into(),
        key_schema: TableKeySchema {
            hash_keys: v(hash),
            range_keys: v(range),
        },
        ..Default::default()
    };
    t.local_secondary_indexes
        .push(idx("ne_lsi", &["pk"], &["l"]));
    t.global_secondary_indexes
        .push(idx("ne_gsi", &["g"], &["h"]));
    t.global_secondary_indexes
        .push(idx("ne_cgsi", &["g", "q"], &["h", "r"]));
    t.attribute_definitions = [
        ("pk", ScalarType::N),
        ("sk", ScalarType::N),
        ("l", ScalarType::N),
        ("g", ScalarType::S),
        ("h", ScalarType::N),
        ("q", ScalarType::S),
        ("r", ScalarType::N),
    ]
    .into_iter()
    .map(|(n, ty)| AttributeDef { name: n.into(), ty })
    .collect();
    t
}

fn parse_command(sql: &str) -> CommandNode {
    parse_one(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
        .command
}

fn select_cmd(sql: &str) -> SelectCmd {
    match parse_command(sql) {
        CommandNode::Select(c) => c,
        other => panic!("{sql}: not a select: {other:?}"),
    }
}

fn plan_ok(client: &mut FakeDdbClient, sql: &str) -> SelectPlan {
    plan_select(&select_cmd(sql), client, "aaa").unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// The two legs differ only in the key condition.
fn assert_legs_match(lower: &QueryIn, upper: &QueryIn) {
    assert_eq!(lower.table_name, upper.table_name);
    assert_eq!(lower.index_name, upper.index_name);
    assert_eq!(lower.filter_expression, upper.filter_expression);
    assert_eq!(lower.projection_expression, upper.projection_expression);
    assert_eq!(
        lower.expression_attribute_names,
        upper.expression_attribute_names
    );
    assert_eq!(
        lower.expression_attribute_values,
        upper.expression_attribute_values
    );
    assert_eq!(lower.consistent_read, upper.consistent_read);
    assert_eq!(
        lower.return_consumed_capacity,
        upper.return_consumed_capacity
    );
    assert!(upper.exclusive_start_key.is_none());
}

#[test]
fn planner_split_and_no_split_table() {
    use SelectApi::{GetItem, Query, Scan};
    let cases: &[(&str, SelectApi, bool, &str)] = &[
        (
            "select * from ddbsh_ne where pk = 1 and sk <> 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk != 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and not sk = 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne where sk <> 5 and pk = 1;",
            Query,
            true,
            "#aaaa1 < :vaaa1 AND #aaaa2 = :vaaa2",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> 5 and x = 3;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and x <> 3;",
            Query,
            false,
            "#aaaa1 = :vaaa1",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and not sk <> 5;",
            GetItem,
            false,
            "",
        ),
        ("select * from ddbsh_ne where pk <> 1;", Scan, false, ""),
        (
            "select * from ddbsh_ne where pk <> 1 and sk = 5;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> 5 and sk > 2;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> 5 and sk <> 7;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> 5 and begins_with(sk, \"1\");",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and (sk <> 5 or x = 1);",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where not (pk = 1 and sk <> 5);",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> \"x\";",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk <> null;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne where pk = 1 and sk is not null;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne.ne_lsi where pk = 1 and l <> 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne.ne_gsi where g = \"a\" and h <> 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
        ),
        (
            "select * from ddbsh_ne.ne_gsi where g = \"a\" and sk <> 5;",
            Query,
            false,
            "#aaaa1 = :vaaa1",
        ),
        (
            "select * from ddbsh_ne.ne_cgsi where g = \"a\" and q = \"z\" and h <> 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 = :vaaa2 AND #aaaa3 < :vaaa3",
        ),
        (
            "select * from ddbsh_ne.ne_cgsi where g = \"a\" and q = \"z\" and h = 5 and r <> 5;",
            Query,
            true,
            "#aaaa1 = :vaaa1 AND #aaaa2 = :vaaa2 AND #aaaa3 = :vaaa3 AND #aaaa4 < :vaaa4",
        ),
        (
            "select * from ddbsh_ne.ne_cgsi where g = \"a\" and q = \"z\" and h <> 5 and r = 5;",
            Scan,
            false,
            "",
        ),
        (
            "select * from ddbsh_ne.ne_cgsi where g = \"a\" and h <> 5;",
            Scan,
            false,
            "",
        ),
    ];
    for &(sql, api, split, lower_kc) in cases {
        let mut client = FakeDdbClient::default();
        client.set_table(ne_table());
        let plan = plan_ok(&mut client, sql);
        assert_eq!(plan.api, api, "{sql}");
        assert_eq!(plan.query_upper.is_some(), split, "{sql}");
        if api != Query {
            continue;
        }
        assert_eq!(plan.query.key_condition_expression, lower_kc, "{sql}");
        if split {
            let upper = plan.query_upper.as_ref().unwrap();
            assert_eq!(
                upper.key_condition_expression,
                lower_kc.replacen(" < ", " > ", 1),
                "{sql}"
            );
            assert_legs_match(&plan.query, upper);
        }
    }
}

#[test]
fn planner_legs_share_filter_projection_consistency_and_capacity() {
    let mut client = FakeDdbClient::default();
    client.set_table(ne_table());
    let plan = plan_ok(
        &mut client,
        "select consistent x, sk from ddbsh_ne where pk = 1 and sk <> 5 and x <> 30 \
         return consumed capacity total;",
    );
    assert_eq!(plan.api, SelectApi::Query);
    let lo = &plan.query;
    let hi = plan.query_upper.as_ref().unwrap();
    assert_eq!(lo.projection_expression.as_deref(), Some("#aaaa1, #aaaa2"));
    assert_eq!(
        lo.key_condition_expression,
        "#aaaa3 = :vaaa1 AND #aaaa2 < :vaaa2"
    );
    assert_eq!(lo.filter_expression.as_deref(), Some("#aaaa1 <> :vaaa3"));
    assert!(lo.consistent_read);
    assert_eq!(lo.return_consumed_capacity, ReturnConsumedCapacity::Total);
    assert_eq!(
        hi.key_condition_expression,
        "#aaaa3 = :vaaa1 AND #aaaa2 > :vaaa2"
    );
    assert_legs_match(lo, hi);
}

#[test]
fn planner_bang_filter_keeps_its_spelling_on_both_legs() {
    let mut client = FakeDdbClient::default();
    client.set_table(ne_table());
    let plan = plan_ok(
        &mut client,
        "select * from ddbsh_ne where pk = 1 and sk != 5 and x != 30;",
    );
    let hi = plan.query_upper.as_ref().unwrap();
    assert_eq!(
        plan.query.filter_expression.as_deref(),
        Some("NOT (#aaaa3 = :vaaa3)")
    );
    assert_legs_match(&plan.query, hi);
}

#[test]
fn planner_index_legs_carry_index_name() {
    let mut client = FakeDdbClient::default();
    client.set_table(ne_table());
    let plan = plan_ok(
        &mut client,
        "select * from ddbsh_ne.ne_gsi where g = \"a\" and h <> 5;",
    );
    assert_eq!(plan.query.index_name.as_deref(), Some("ne_gsi"));
    assert_eq!(
        plan.query_upper.unwrap().index_name.as_deref(),
        Some("ne_gsi")
    );
}

#[test]
fn planner_no_declared_type_splits_any_scalar() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    let plan = plan_ok(&mut client, "select * from t where a = 1 and b <> \"x\";");
    assert_eq!(plan.api, SelectApi::Query);
    assert!(plan.query_upper.is_some());
}

// ---- execution ---------------------------------------------------------------

fn kces(client: &FakeDdbClient) -> Vec<String> {
    client
        .query_requests
        .iter()
        .map(|q| q.key_condition_expression.clone())
        .collect()
}

fn explain_query_kces(client: &FakeDdbClient) -> Vec<String> {
    client
        .explain_requests
        .iter()
        .map(|r| match r {
            ExplainRequest::Query(q) => q.key_condition_expression.clone(),
            other => panic!("not a Query: {other:?}"),
        })
        .collect()
}

#[test]
fn explain_select_prints_two_query_blocks() {
    for op in ["<>", "!="] {
        let mut client = FakeDdbClient::default();
        client.set_table(table("t", "a", "b"));
        let cmd = select_cmd(&format!("select * from t where a = 3 and b {op} 4;"));
        let ExecOutcome::Explain(e) = execute_select_explain(&cmd, &mut client, "aaa").unwrap()
        else {
            panic!()
        };
        let apis: Vec<&str> = e.blocks.iter().map(|b| b.api_name.as_str()).collect();
        assert_eq!(apis, ["Query", "Query"], "{op}");
        assert_eq!(
            explain_query_kces(&client),
            [
                "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2",
                "#aaaa1 = :vaaa1 AND #aaaa2 > :vaaa2"
            ]
        );
    }
}

#[test]
fn split_select_reads_both_legs_in_order() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.pages = vec![
        vec![row(&[("a", num("1")), ("b", num("1"))])],
        vec![row(&[("a", num("1")), ("b", num("2"))])],
    ];
    let cmd = select_cmd("select * from t where a = 1 and b <> 5;");
    let mut cursor = open_select_cursor(&cmd, &mut client, "aaa").unwrap();
    let mut pages = 0;
    while cursor.next_page(&mut client).unwrap().is_some() {
        pages += 1;
    }
    // Two pages per leg; the upper leg starts from its first page.
    assert_eq!(pages, 4);
    let lower = "#aaaa1 = :vaaa1 AND #aaaa2 < :vaaa2";
    let upper = "#aaaa1 = :vaaa1 AND #aaaa2 > :vaaa2";
    assert_eq!(kces(&client), [lower, lower, upper, upper]);
    assert!(client.query_requests[2].exclusive_start_key.is_none());
}

#[test]
fn split_select_stopped_during_leg_one_never_starts_leg_two() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.set_items(vec![row(&[("a", num("1")), ("b", num("1"))])]);
    let cmd = select_cmd("select * from t where a = 1 and b <> 5;");
    let mut cursor = open_select_cursor(&cmd, &mut client, "aaa").unwrap();
    assert!(cursor.next_page(&mut client).unwrap().is_some());
    assert_eq!(client.query_requests.len(), 1);
}

#[test]
fn split_bulk_update_enumerates_both_legs_and_sums_the_counts() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.set_items(vec![
        row(&[("a", num("1")), ("b", num("1"))]),
        row(&[("a", num("1")), ("b", num("2"))]),
    ]);
    let CommandNode::Update(cmd) = parse_command("update t set x = 1 where a = 1 and b <> 5;")
    else {
        panic!()
    };
    let mut sink = BufferingSink::default();
    execute_update(&cmd, &mut client, &mut gen(), false, &mut sink).unwrap();
    assert_eq!(client.query_requests.len(), 2);
    assert!(client.query_requests[0]
        .key_condition_expression
        .contains(" < "));
    assert!(client.query_requests[1]
        .key_condition_expression
        .contains(" > "));
    assert_eq!(client.update_requests.len(), 4);
    let Some(ExecOutcome::BulkWrite(acct)) = sink.parts.last() else {
        panic!("{:?}", sink.parts)
    };
    assert_eq!((acct.read, acct.modified, acct.ccf), (4, 4, 0));
}

#[test]
fn split_bulk_delete_enumerates_both_legs() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.set_items(vec![row(&[("a", num("1")), ("b", num("1"))])]);
    let CommandNode::Delete(cmd) = parse_command("delete from t where a = 1 and b != 5;") else {
        panic!()
    };
    let mut sink = BufferingSink::default();
    execute_delete(&cmd, &mut client, &mut gen(), false, &mut sink).unwrap();
    assert_eq!(client.query_requests.len(), 2);
    assert_eq!(client.delete_item_requests.len(), 2);
    let Some(ExecOutcome::BulkWrite(acct)) = sink.parts.last() else {
        panic!("{:?}", sink.parts)
    };
    assert_eq!((acct.read, acct.modified), (2, 2));
}

fn explain_apis(sink: &BufferingSink) -> Vec<String> {
    sink.parts
        .iter()
        .flat_map(|p| match p {
            ExecOutcome::Explain(e) => e
                .blocks
                .iter()
                .map(|b| b.api_name.clone())
                .collect::<Vec<_>>(),
            other => panic!("not explain: {other:?}"),
        })
        .collect()
}

#[test]
fn split_explain_bulk_update_prints_each_legs_blocks_in_order() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.set_items(vec![row(&[("a", num("1")), ("b", num("1"))])]);
    let CommandNode::Update(cmd) = parse_command("update t set x = 1 where a = 1 and b <> 5;")
    else {
        panic!()
    };
    let mut sink = BufferingSink::default();
    execute_update(&cmd, &mut client, &mut gen(), true, &mut sink).unwrap();
    assert_eq!(
        explain_apis(&sink),
        ["Query", "UpdateItem", "Query", "UpdateItem"]
    );
    assert!(client.update_requests.is_empty());
}

#[test]
fn split_explain_bulk_delete_prints_both_query_blocks_and_runs_nothing() {
    let mut client = FakeDdbClient::default();
    client.set_table(table("t", "a", "b"));
    client.set_items(vec![row(&[("a", num("1")), ("b", num("1"))])]);
    let CommandNode::Delete(cmd) = parse_command("delete from t where a = 1 and b <> 5;") else {
        panic!()
    };
    let mut sink = BufferingSink::default();
    execute_delete(&cmd, &mut client, &mut gen(), true, &mut sink).unwrap();
    assert_eq!(explain_apis(&sink), ["Query", "Query"]);
    assert_eq!(client.read_calls, 0);
    let kces: Vec<String> = client
        .explain_requests
        .iter()
        .map(|r| match r {
            ExplainRequest::Query(q) => q.key_condition_expression.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(kces[0].contains(" < ") && kces[1].contains(" > "));
}

#[test]
fn transact_read_with_sort_key_not_equal_aborts_in_both_spellings() {
    for op in ["<>", "!="] {
        let mut client = FakeDdbClient::default();
        client.set_table(table("t", "a", "b"));
        let txn = TransactCmd {
            access: TransactAccess::Read,
            rollback: false,
            items: vec![CommandNode::Select(select_cmd(&format!(
                "select * from t where a = 1 and b {op} 2;"
            )))],
        };
        let mut sink = BufferingSink::default();
        execute_transact(&txn, &mut client, &mut gen(), false, &mut sink).unwrap();
        assert_eq!(
            sink.parts,
            vec![ExecOutcome::Transact(TransactOutcome {
                token: "ABORT".into(),
                diagnostics: vec![
                    crate::execute::transact_build::SELECT_NOT_REDUCIBLE_MESSAGE.into()
                ]
            })],
            "{op}"
        );
        assert!(client.transact_get_requests.is_empty());
    }
}
