//! Offline fake DynamoDB client (port of tests/fake_ddb_client.hpp, the parts
//! the DML executors need) plus the DML/planner unit tests.

use std::collections::BTreeMap;

use crate::client::*;
use crate::error::{fail, Errc, Error, Result};
use crate::value::{Value, ValueMap};

pub fn make_key_schema(hash: &str, range: &str) -> TableKeySchema {
    TableKeySchema {
        hash_keys: vec![hash.to_string()],
        range_keys: if range.is_empty() {
            vec![]
        } else {
            vec![range.to_string()]
        },
    }
}

pub fn table(name: &str, hash: &str, range: &str) -> DescribeTableOut {
    DescribeTableOut {
        table_name: name.into(),
        key_schema: make_key_schema(hash, range),
        ..Default::default()
    }
}

#[derive(Default)]
pub struct FakeDdbClient {
    pub tables: BTreeMap<String, DescribeTableOut>,
    pub get_item: Option<ItemRow>,
    pub pages: Vec<Vec<ItemRow>>,
    pub consumed_capacity: Option<ConsumedCapacity>,
    pub write_consumed_capacity: Option<ConsumedCapacity>,
    pub read_calls: usize,
    pub put_error: Option<Error>,
    pub put_error_at: Option<usize>,
    pub update_error: Option<Error>,
    pub delete_item_error: Option<Error>,
    pub put_attributes: Option<ItemRow>,
    pub update_attributes: Option<ItemRow>,
    pub delete_attributes: Option<ItemRow>,
    pub put_requests: Vec<PutItemIn>,
    pub update_requests: Vec<UpdateItemIn>,
    pub delete_item_requests: Vec<DeleteItemIn>,
    pub query_requests: Vec<QueryIn>,
    pub scan_requests: Vec<ScanIn>,
    pub get_requests: Vec<GetItemIn>,
    pub search_vectors_out: SearchVectorsOut,
    pub search_vectors_requests: Vec<SearchVectorsIn>,
    pub transact_get_out: Option<TransactGetItemsOut>,
    pub transact_get_requests: Vec<TransactGetItemsIn>,
    pub transact_write_requests: Vec<TransactWriteItemsIn>,
    pub transact_write_error: Option<Error>,
    pub explain_requests: Vec<ExplainRequest>,
}

impl FakeDdbClient {
    pub fn set_table(&mut self, t: DescribeTableOut) {
        self.tables.insert(t.table_name.clone(), t);
    }
    pub fn set_items(&mut self, items: Vec<ItemRow>) {
        self.pages = vec![items];
    }
    fn page_index(start_key: &Option<ValueMap>) -> usize {
        start_key
            .as_ref()
            .and_then(|k| k.iter().find(|(n, _)| n == "__page"))
            .and_then(|(_, v)| {
                if let Value::N(s) = v {
                    s.parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }
    fn next_cursor(&self, page: usize) -> Option<ValueMap> {
        if page + 1 >= self.pages.len() {
            None
        } else {
            Some(vec![("__page".into(), Value::N((page + 1).to_string()))])
        }
    }
    fn page(&mut self, start: &Option<ValueMap>) -> (Vec<ItemRow>, Option<ValueMap>) {
        self.read_calls += 1;
        let p = Self::page_index(start);
        (
            self.pages.get(p).cloned().unwrap_or_default(),
            self.next_cursor(p),
        )
    }
}

fn nyi<T>() -> Result<T> {
    fail(Errc::NotImplemented, "fake: not implemented")
}

impl DdbClient for FakeDdbClient {
    fn get_item(&mut self, input: &GetItemIn) -> Result<GetItemOut> {
        self.get_requests.push(input.clone());
        Ok(GetItemOut {
            item: self.get_item.clone(),
            consumed_capacity: self.consumed_capacity.clone(),
        })
    }
    fn query(&mut self, input: &QueryIn) -> Result<QueryOut> {
        self.query_requests.push(input.clone());
        let (items, lek) = self.page(&input.exclusive_start_key);
        Ok(QueryOut {
            items,
            last_evaluated_key: lek,
            consumed_capacity: self.consumed_capacity.clone(),
        })
    }
    fn scan(&mut self, input: &ScanIn) -> Result<ScanOut> {
        self.scan_requests.push(input.clone());
        let (items, lek) = self.page(&input.exclusive_start_key);
        Ok(ScanOut {
            items,
            last_evaluated_key: lek,
            consumed_capacity: self.consumed_capacity.clone(),
        })
    }
    fn search_vectors(&mut self, input: &SearchVectorsIn) -> Result<SearchVectorsOut> {
        self.search_vectors_requests.push(input.clone());
        Ok(self.search_vectors_out.clone())
    }
    fn put_item(&mut self, input: &PutItemIn) -> Result<PutItemOut> {
        let call = self.put_requests.len();
        self.put_requests.push(input.clone());
        if let Some(e) = &self.put_error {
            if self.put_error_at.map(|at| at == call).unwrap_or(true) {
                return Err(e.clone());
            }
        }
        Ok(PutItemOut {
            attributes: self.put_attributes.clone(),
            consumed_capacity: self.write_consumed_capacity.clone(),
        })
    }
    fn update_item(&mut self, input: &UpdateItemIn) -> Result<UpdateItemOut> {
        self.update_requests.push(input.clone());
        if let Some(e) = &self.update_error {
            return Err(e.clone());
        }
        Ok(UpdateItemOut {
            attributes: self.update_attributes.clone(),
            consumed_capacity: self.write_consumed_capacity.clone(),
        })
    }
    fn delete_item(&mut self, input: &DeleteItemIn) -> Result<DeleteItemOut> {
        self.delete_item_requests.push(input.clone());
        if let Some(e) = &self.delete_item_error {
            return Err(e.clone());
        }
        Ok(DeleteItemOut {
            attributes: self.delete_attributes.clone(),
            consumed_capacity: self.write_consumed_capacity.clone(),
        })
    }
    fn describe_table(&mut self, input: &DescribeTableIn) -> Result<DescribeTableOut> {
        match self.tables.get(&input.table_name) {
            Some(t) => Ok(t.clone()),
            None => fail(
                Errc::NoSuchTable,
                format!("Table: {} not found", input.table_name),
            ),
        }
    }
    fn transact_get_items(&mut self, input: &TransactGetItemsIn) -> Result<TransactGetItemsOut> {
        self.transact_get_requests.push(input.clone());
        Ok(self.transact_get_out.clone().unwrap_or_default())
    }
    fn transact_write_items(&mut self, input: &TransactWriteItemsIn) -> Result<()> {
        self.transact_write_requests.push(input.clone());
        match &self.transact_write_error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
    fn explain_request(&mut self, request: &ExplainRequest) -> Result<String> {
        self.explain_requests.push(request.clone());
        Ok("{\n\t\"stub\":\t\"explain\"\n}".to_string())
    }
    fn create_table(&mut self, _: &CreateTableIn) -> Result<()> {
        nyi()
    }
    fn update_table(&mut self, _: &UpdateTableIn) -> Result<()> {
        nyi()
    }
    fn update_time_to_live(&mut self, _: &UpdateTimeToLiveIn) -> Result<()> {
        nyi()
    }
    fn update_continuous_backups(&mut self, _: &UpdateContinuousBackupsIn) -> Result<()> {
        nyi()
    }
    fn create_backup(&mut self, _: &CreateBackupIn) -> Result<()> {
        nyi()
    }
    fn delete_backup(&mut self, _: &DeleteBackupIn) -> Result<()> {
        nyi()
    }
    fn describe_backup(&mut self, _: &DescribeBackupIn) -> Result<DescribeBackupOut> {
        nyi()
    }
    fn restore_table_from_backup(&mut self, _: &RestoreFromBackupIn) -> Result<()> {
        nyi()
    }
    fn restore_table_to_point_in_time(&mut self, _: &RestoreToPitrIn) -> Result<()> {
        nyi()
    }
    fn describe_limits(&mut self, _: &DescribeLimitsIn) -> Result<DescribeLimitsOut> {
        nyi()
    }
    fn list_backups(&mut self, _: &ListBackupsIn) -> Result<ListBackupsOut> {
        nyi()
    }
    fn delete_table(&mut self, _: &DeleteTableIn) -> Result<()> {
        nyi()
    }
    fn list_tables(&mut self, _: &ListTablesIn) -> Result<ListTablesOut> {
        nyi()
    }
    fn describe_time_to_live(&mut self, _: &DescribeTimeToLiveIn) -> Result<TimeToLiveOut> {
        nyi()
    }
    fn describe_continuous_backups(
        &mut self,
        _: &DescribeContinuousBackupsIn,
    ) -> Result<ContinuousBackupsOut> {
        nyi()
    }
    fn describe_table_replica_auto_scaling(
        &mut self,
        _: &DescribeTableReplicaAutoScalingIn,
    ) -> Result<TableReplicaAutoScalingOut> {
        nyi()
    }
    fn wait_for_table_active(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn wait_for_table_gone(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::delete_executor::execute_delete;
    use crate::execute::insert_executor::{execute_insert_explain, execute_insert_streaming};
    use crate::execute::rate_limiter::RateLimiter;
    use crate::execute::select_executor::*;
    use crate::execute::transact_executor::execute_transact;
    use crate::execute::update_executor::execute_update;
    use crate::expressions::{unique_prefix, LogicalExpr, Where};
    use crate::outcome::*;
    use crate::parser::ast::*;

    fn n(s: &str) -> Value {
        Value::N(s.into())
    }
    fn eq(lhs: &str, v: Value) -> LogicalExpr {
        LogicalExpr::comparison(lhs, "=", v)
    }
    fn and(a: LogicalExpr, b: LogicalExpr) -> LogicalExpr {
        LogicalExpr::make_and(a, b)
    }
    fn row(pairs: &[(&str, Value)]) -> ItemRow {
        ItemRow {
            attributes: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }
    fn names(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }
    fn select(table: &str, w: Option<LogicalExpr>) -> SelectCmd {
        SelectCmd {
            table: table.into(),
            where_: w.map(Where::new),
            ..Default::default()
        }
    }
    fn gen() -> impl FnMut() -> String {
        let mut c = 0u64;
        move || {
            let p = unique_prefix(c);
            c += 1;
            p
        }
    }
    fn explain_api(o: &ExecOutcome) -> Vec<String> {
        match o {
            ExecOutcome::Explain(e) => e.blocks.iter().map(|b| b.api_name.clone()).collect(),
            other => panic!("not explain: {:?}", other),
        }
    }

    #[test]
    fn select_full_key_is_getitem() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let o = execute_select_explain(&select("t", Some(eq("a", n("3")))), &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["GetItem"]);
        match &c.explain_requests[0] {
            ExplainRequest::GetItem(g) => {
                assert_eq!(g.key, vec![("a".to_string(), n("3"))]);
                assert!(g.expression_attribute_names.is_empty());
            }
            r => panic!("{:?}", r),
        }
    }

    #[test]
    fn select_hash_only_with_range_is_query_and_no_where_is_scan() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", "b"));
        let o = execute_select_explain(&select("t", Some(eq("a", n("3")))), &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["Query"]);
        let o = execute_select_explain(&select("t", None), &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["Scan"]);
    }

    #[test]
    fn select_missing_table_is_error() {
        let mut c = FakeDdbClient::default();
        assert!(execute_select_explain(&select("nope", None), &mut c, "aaa").is_err());
    }

    /// Golden ddbsh-basic-test-8: `where a = 3 and b = 4` on a hash-only table.
    #[test]
    fn golden_basic_test_8_query_shape() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("ddbsh_test_table_1", "a", ""));
        let cmd = select(
            "ddbsh_test_table_1",
            Some(and(eq("a", n("3")), eq("b", n("4")))),
        );
        execute_select_explain(&cmd, &mut c, "aaa").unwrap();
        let ExplainRequest::Query(q) = &c.explain_requests[0] else {
            panic!()
        };
        assert_eq!(q.key_condition_expression, "#aaaa1 = :vaaa1");
        assert_eq!(q.filter_expression.as_deref(), Some("#aaaa2 = :vaaa2"));
        assert_eq!(
            q.expression_attribute_names,
            names(&[("#aaaa1", "a"), ("#aaaa2", "b")])
        );
        assert_eq!(
            q.expression_attribute_values,
            vec![
                (":vaaa1".to_string(), n("3")),
                (":vaaa2".to_string(), n("4"))
            ]
        );
        assert!(!q.consistent_read);
        assert_eq!(q.return_consumed_capacity, ReturnConsumedCapacity::None);
    }

    fn composite_gsi() -> DescribeTableOut {
        let mut t = table("t", "a", "");
        t.global_secondary_indexes.push(SecondaryIndexSchema {
            index_name: "firstgsi".into(),
            key_schema: TableKeySchema {
                hash_keys: vec!["a".into(), "b".into(), "c".into()],
                range_keys: vec!["d".into(), "e".into()],
            },
            ..Default::default()
        });
        t
    }

    #[test]
    fn composite_gsi_partial_hash_scan_full_hash_query() {
        let mut c = FakeDdbClient::default();
        c.set_table(composite_gsi());
        let mk = |w: LogicalExpr| SelectCmd {
            table: "t".into(),
            index: Some("firstgsi".into()),
            where_: Some(Where::new(w)),
            ..Default::default()
        };
        let o = execute_select_explain(&mk(and(eq("a", n("3")), eq("b", n("4")))), &mut c, "aaa")
            .unwrap();
        assert_eq!(explain_api(&o), ["Scan"]);
        let full = and(and(eq("a", n("3")), eq("b", n("4"))), eq("c", n("5")));
        let o = execute_select_explain(&mk(full.clone()), &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["Query"]);
        let o = execute_select_explain(&mk(and(full, eq("d", n("6")))), &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["Query"]);
        let ExplainRequest::Query(q) = c.explain_requests.last().unwrap() else {
            panic!()
        };
        assert_eq!(q.index_name.as_deref(), Some("firstgsi"));
        assert_eq!(
            q.key_condition_expression,
            "#aaaa1 = :vaaa1 AND #aaaa2 = :vaaa2 AND #aaaa3 = :vaaa3 AND #aaaa4 = :vaaa4"
        );
        assert!(q.filter_expression.is_none());
    }

    #[test]
    fn select_streaming_pages_and_capacity_total() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        c.pages = vec![
            vec![row(&[("a", n("1"))])],
            vec![],
            vec![row(&[("a", n("2"))]), row(&[("a", n("3"))])],
        ];
        c.consumed_capacity = Some(ConsumedCapacity {
            table_name: Some("t".into()),
            capacity_units: Some(0.5),
            ..Default::default()
        });
        let cmd = SelectCmd {
            rcc: ReturnConsumedCapacity::Total,
            ..select("t", None)
        };
        let mut sink = BufferingSink::default();
        execute_select_streaming(&cmd, &mut c, "aaa", &mut sink).unwrap();
        assert_eq!(c.read_calls, 3);
        assert_eq!(sink.parts.len(), 3); // two non-empty pages + capacity fragment
        let ExecOutcome::Select(last) = &sink.parts[2] else {
            panic!()
        };
        assert!(last.items.is_empty());
        assert_eq!(
            last.consumed_capacity.as_ref().unwrap().capacity_units,
            Some(1.5)
        );
        let o = execute_select(&cmd, &mut c, "aaa").unwrap();
        let ExecOutcome::Select(s) = o else { panic!() };
        assert_eq!(s.items.len(), 3);
    }

    #[test]
    fn select_ratelimit_forces_total_capacity_but_not_reported() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let cmd = SelectCmd {
            ratelimit: Some(RateLimit {
                rcu: 100.0,
                wcu: 0.0,
            }),
            ..select("t", None)
        };
        execute_select_explain(&cmd, &mut c, "aaa").unwrap();
        let ExplainRequest::Scan(s) = &c.explain_requests[0] else {
            panic!()
        };
        assert_eq!(s.return_consumed_capacity, ReturnConsumedCapacity::Total);
    }

    #[test]
    fn vector_select_requires_nearest() {
        let mut c = FakeDdbClient::default();
        let mut t = table("t", "a", "");
        t.vector_indexes.push(VectorIndexSchema {
            index_name: "vi".into(),
            dimensions: 2,
            ..Default::default()
        });
        c.set_table(t);
        let mut cmd = SelectCmd {
            index: Some("vi".into()),
            ..select("t", None)
        };
        let e = execute_select_explain(&cmd, &mut c, "aaa").unwrap_err();
        assert_eq!(
            e.message,
            "A SELECT on vector index vi requires a NEAREST <n> TO [ ... ] clause."
        );
        cmd.search_vector = Some(Value::L(vec![n("1")]));
        cmd.topk = Some(3);
        let e = execute_select_explain(&cmd, &mut c, "aaa").unwrap_err();
        assert_eq!(
            e.message,
            "The search vector has 1 values; vector index vi expects 2."
        );
        cmd.search_vector = Some(Value::L(vec![n("1"), n("2")]));
        let o = execute_select_explain(&cmd, &mut c, "aaa").unwrap();
        assert_eq!(explain_api(&o), ["SearchVectors"]);
    }

    fn insert_cmd(rows: usize) -> InsertCmd {
        InsertCmd {
            table_name: "t".into(),
            columns: vec!["a".into(), "b".into()],
            rows: (0..rows).map(|i| vec![n(&i.to_string()), n("9")]).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn insert_explain_one_block_per_row_with_fresh_prefixes() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let mut g = gen();
        let o = execute_insert_explain(&insert_cmd(3), &mut c, &mut g).unwrap();
        assert_eq!(explain_api(&o), ["PutItem", "PutItem", "PutItem"]);
        let conds: Vec<_> = c
            .explain_requests
            .iter()
            .map(|r| match r {
                ExplainRequest::PutItem(p) => (
                    p.condition_expression.clone().unwrap(),
                    p.expression_attribute_names.clone(),
                ),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            conds[0],
            (
                "attribute_not_exists(#aaaa1)".to_string(),
                names(&[("#aaaa1", "a")])
            )
        );
        assert_eq!(conds[1].0, "attribute_not_exists(#abaa1)");
        assert_eq!(conds[2].0, "attribute_not_exists(#acaa1)");
    }

    #[test]
    fn insert_unequal_columns_is_error_and_replace_has_no_condition() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let mut cmd = insert_cmd(2);
        cmd.rows[1].pop();
        let e = execute_insert_explain(&cmd, &mut c, &mut gen()).unwrap_err();
        assert_eq!(
            e.message,
            "Unequal number of column names (2) and values (1) in tuple 2"
        );
        let cmd = InsertCmd {
            insert: false,
            ..insert_cmd(2)
        };
        let mut sink = BufferingSink::default();
        execute_insert_streaming(&cmd, &mut c, &mut gen(), &mut sink).unwrap();
        assert!(c
            .put_requests
            .iter()
            .all(|p| p.condition_expression.is_none()));
        assert_eq!(sink.parts.len(), 2);
        let ExecOutcome::Write(w) = &sink.parts[0] else {
            panic!()
        };
        assert_eq!(w.tokens, ["REPLACE"]);
        assert!(w.items.is_empty());
    }

    #[test]
    fn insert_error_stops_after_earlier_rows_emitted() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        c.put_error = Some(Error::new(
            Errc::ConditionalCheckFailed,
            "The conditional request failed",
        ));
        c.put_error_at = Some(1);
        let mut sink = BufferingSink::default();
        let r = execute_insert_streaming(&insert_cmd(3), &mut c, &mut gen(), &mut sink);
        assert!(r.is_err());
        assert_eq!(sink.parts.len(), 1);
        assert_eq!(c.put_requests.len(), 2);
    }

    fn update_cmd(w: Option<LogicalExpr>) -> UpdateCmd {
        UpdateCmd {
            table: "t".into(),
            set: vec![UpdateSetElement {
                lhs: "c".into(),
                op: "+".into(),
                rhs_attribute: "c".into(),
                rhs_value: Some(n("1")),
                ..Default::default()
            }],
            where_: w.map(Where::new),
            ..Default::default()
        }
    }

    #[test]
    fn update_point_explain_shape() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let cmd = update_cmd(Some(and(
            eq("a", n("1")),
            LogicalExpr::comparison("x", ">", n("3")),
        )));
        let mut sink = BufferingSink::default();
        execute_update(&cmd, &mut c, &mut gen(), true, &mut sink).unwrap();
        assert_eq!(explain_api(&sink.parts[0]), ["UpdateItem"]);
        let ExplainRequest::UpdateItem(u) = &c.explain_requests[0] else {
            panic!()
        };
        assert_eq!(u.update_expression, "SET #aaaa1 = #aaaa1+:vaaa1");
        assert_eq!(
            u.condition_expression.as_deref(),
            Some("attribute_exists(#aaaa2) AND #aaaa3 > :vaaa2")
        );
        assert_eq!(u.key, vec![("a".to_string(), n("1"))]);
        assert!(c.update_requests.is_empty());
    }

    #[test]
    fn update_bulk_counts_read_modified_ccf() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        c.pages = vec![
            vec![row(&[("a", n("1"))]), row(&[("a", n("2"))])],
            vec![row(&[("a", n("3"))])],
        ];
        let mut sink = BufferingSink::default();
        execute_update(&update_cmd(None), &mut c, &mut gen(), false, &mut sink).unwrap();
        let ExecOutcome::BulkWrite(b) = &sink.parts[0] else {
            panic!()
        };
        assert_eq!(
            (b.read, b.modified, b.ccf, b.verb),
            (3, 3, 0, BulkVerb::Update)
        );
        // read uses the second prefix; projection is the base keys
        assert_eq!(
            c.scan_requests[0].projection_expression.as_deref(),
            Some("#abaa1")
        );
        assert_eq!(c.update_requests[2].key, vec![("a".to_string(), n("3"))]);

        c.update_error = Some(Error::new(Errc::ConditionalCheckFailed, "ccf"));
        let mut sink = BufferingSink::default();
        execute_update(&update_cmd(None), &mut c, &mut gen(), false, &mut sink).unwrap();
        let ExecOutcome::BulkWrite(b) = &sink.parts[0] else {
            panic!()
        };
        assert_eq!((b.read, b.modified, b.ccf), (3, 0, 3));

        c.update_error = Some(Error::new(Errc::ServiceError, "boom"));
        let mut sink = BufferingSink::default();
        assert!(execute_update(&update_cmd(None), &mut c, &mut gen(), false, &mut sink).is_err());
        assert!(sink.parts.is_empty());
    }

    #[test]
    fn update_explain_bulk_emits_read_then_writes_per_page() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        c.pages = vec![vec![row(&[("a", n("1"))])], vec![row(&[("a", n("2"))])]];
        let mut sink = BufferingSink::default();
        execute_update(&update_cmd(None), &mut c, &mut gen(), true, &mut sink).unwrap();
        assert_eq!(
            explain_api(&sink.parts[0]),
            ["Scan", "UpdateItem", "Scan", "UpdateItem"]
        );
        assert!(c.update_requests.is_empty());
    }

    /// Golden ddbsh-delete-6 and ddbsh-delete-8.
    #[test]
    fn golden_delete_shapes() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("idt1", "pk", ""));
        let cmd = DeleteCmd {
            table: "idt1".into(),
            where_: Some(Where::new(and(eq("pk", n("0")), eq("x", n("3"))))),
            ..Default::default()
        };
        let mut sink = BufferingSink::default();
        execute_delete(&cmd, &mut c, &mut gen(), true, &mut sink).unwrap();
        let ExplainRequest::DeleteItem(d) = &c.explain_requests[0] else {
            panic!()
        };
        assert_eq!(d.key, vec![("pk".to_string(), n("0"))]);
        assert_eq!(d.condition_expression.as_deref(), Some("#aaaa1 = :vaaa1"));
        assert_eq!(d.expression_attribute_names, names(&[("#aaaa1", "x")]));

        c.explain_requests.clear();
        c.pages = vec![vec![
            row(&[("pk", n("3"))]),
            row(&[("pk", n("2"))]),
            row(&[("pk", n("1"))]),
        ]];
        let cmd = DeleteCmd {
            table: "idt1".into(),
            where_: Some(Where::new(LogicalExpr::between("pk", n("1"), n("3")))),
            ..Default::default()
        };
        let mut sink = BufferingSink::default();
        execute_delete(&cmd, &mut c, &mut gen(), true, &mut sink).unwrap();
        assert_eq!(
            explain_api(&sink.parts[0]),
            ["Scan", "DeleteItem", "DeleteItem", "DeleteItem"]
        );
        let ExplainRequest::Scan(s) = &c.explain_requests[0] else {
            panic!()
        };
        assert_eq!(s.projection_expression.as_deref(), Some("#abaa1"));
        assert_eq!(
            s.filter_expression.as_deref(),
            Some("#abaa1 BETWEEN :vbaa1 AND :vbaa2")
        );
        let ExplainRequest::DeleteItem(d) = &c.explain_requests[1] else {
            panic!()
        };
        assert!(d.condition_expression.is_none());
        assert!(d.expression_attribute_names.is_empty());
    }

    #[test]
    fn delete_explain_query_emits_only_query() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", "b"));
        c.pages = vec![vec![row(&[("a", n("1")), ("b", n("2"))])]];
        let cmd = DeleteCmd {
            table: "t".into(),
            where_: Some(Where::new(eq("a", n("1")))),
            ..Default::default()
        };
        let mut sink = BufferingSink::default();
        execute_delete(&cmd, &mut c, &mut gen(), true, &mut sink).unwrap();
        assert_eq!(explain_api(&sink.parts[0]), ["Query"]);
        assert_eq!(c.read_calls, 0);
    }

    #[test]
    fn transact_read_abort_and_success() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let txn = TransactCmd {
            access: TransactAccess::Read,
            rollback: false,
            items: vec![CommandNode::Select(select(
                "t",
                Some(LogicalExpr::comparison("a", ">", n("1"))),
            ))],
        };
        let mut sink = BufferingSink::default();
        execute_transact(&txn, &mut c, &mut gen(), false, &mut sink).unwrap();
        assert_eq!(
            sink.parts,
            vec![ExecOutcome::Transact(TransactOutcome {
                token: "ABORT".into(),
                diagnostics: vec![
                    crate::execute::transact_build::SELECT_NOT_REDUCIBLE_MESSAGE.into()
                ]
            })]
        );
        c.transact_get_out = Some(TransactGetItemsOut {
            responses: vec![None, Some(row(&[("a", n("2"))]))],
        });
        let txn = TransactCmd {
            access: TransactAccess::Read,
            rollback: false,
            items: vec![
                CommandNode::Select(select("t", Some(eq("a", n("1"))))),
                CommandNode::Select(select("t", Some(eq("a", n("2"))))),
            ],
        };
        let mut sink = BufferingSink::default();
        execute_transact(&txn, &mut c, &mut gen(), false, &mut sink).unwrap();
        let ExecOutcome::TransactRead(r) = &sink.parts[0] else {
            panic!()
        };
        assert_eq!(r.items.len(), 2);
        assert!(r.items[0].attributes.is_empty());
    }

    #[test]
    fn transact_write_commit_and_condition_check() {
        let mut c = FakeDdbClient::default();
        c.set_table(table("t", "a", ""));
        let check = SelectCmd {
            exists_mode: ExistsMode::NotExists,
            ..select("t", Some(and(eq("a", n("5")), eq("x", n("1")))))
        };
        let txn = TransactCmd {
            access: TransactAccess::Write,
            rollback: false,
            items: vec![
                CommandNode::Insert(insert_cmd(1)),
                CommandNode::Select(check),
            ],
        };
        let mut sink = BufferingSink::default();
        execute_transact(&txn, &mut c, &mut gen(), false, &mut sink).unwrap();
        assert_eq!(sink.parts, vec![ExecOutcome::status("COMMIT")]);
        let items = &c.transact_write_requests[0].items;
        let TransactWriteItem::ConditionCheck(cc) = &items[1] else {
            panic!()
        };
        assert_eq!(
            cc.condition_expression,
            "#abaa1 = :vbaa1 AND attribute_not_exists(#abaa2)"
        );
        assert_eq!(cc.key, vec![("a".to_string(), n("5"))]);

        // a select without EXISTS aborts with the diagnostic
        let txn = TransactCmd {
            access: TransactAccess::Write,
            rollback: false,
            items: vec![CommandNode::Select(select("t", None))],
        };
        let mut sink = BufferingSink::default();
        execute_transact(&txn, &mut c, &mut gen(), false, &mut sink).unwrap();
        let ExecOutcome::Transact(t) = &sink.parts[0] else {
            panic!()
        };
        assert_eq!(
            t.diagnostics,
            [crate::execute::transact_build::SELECT_NO_EXISTS_MESSAGE]
        );

        // a service failure emits ABORT and propagates the error
        c.transact_write_error = Some(Error::new(Errc::ServiceError, "TransactionCanceled"));
        let txn = TransactCmd {
            access: TransactAccess::Write,
            rollback: false,
            items: vec![CommandNode::Insert(insert_cmd(1))],
        };
        let mut sink = BufferingSink::default();
        assert!(execute_transact(&txn, &mut c, &mut gen(), false, &mut sink).is_err());
        assert_eq!(
            sink.parts,
            vec![ExecOutcome::Transact(TransactOutcome::default())]
        );
    }

    #[test]
    fn rate_limiter_sleeps_for_deficit() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let now = Rc::new(RefCell::new(0.0f64));
        let slept = Rc::new(RefCell::new(Vec::<f64>::new()));
        let (n1, n2, s1) = (now.clone(), now.clone(), slept.clone());
        let mut l = RateLimiter::with_clock(
            10.0,
            0.0,
            Box::new(move || *n1.borrow()),
            Box::new(move |d| {
                s1.borrow_mut().push(d);
                *n2.borrow_mut() += d;
            }),
        );
        l.consume_reads(&Some(ConsumedCapacity {
            capacity_units: Some(5.0),
            ..Default::default()
        }));
        l.readlimit();
        assert_eq!(*slept.borrow(), vec![0.5]);
        l.writelimit(); // wcu 0 => never sleeps
        assert_eq!(slept.borrow().len(), 1);
    }
}
