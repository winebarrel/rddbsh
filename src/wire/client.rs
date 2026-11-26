// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! `WireClient`: the `DdbClient` implemented over a JSON `Transport`, with
//! transient-error retries and the table-state waiters (port of wire/wire_client.cpp).

use std::time::Duration;

use super::codec::{self, WireRequest};
use super::datetime::iso8601_to_epoch;
use super::parse;
use crate::client::*;
use crate::error::{fail, Errc, Error, Result};
use crate::retry::{is_throttling_error, RetryEvent, RetryObserver};

/// Sends one DynamoDB JSON 1.0 request and returns the raw response body.
///
/// A service error must come back as `Ok(body)` carrying the `__type` error
/// document (so it can be parsed/retried); `Err` is reserved for transport
/// failures (unreachable endpoint, TLS, credentials, ...).
pub trait Transport {
    fn send(&mut self, operation: &str, body: &str) -> Result<String>;
    /// Re-target the transport (CONNECT). `endpoint` empty => the region's default.
    fn on_connect(&mut self, _region: &str, _endpoint: &str) -> Result<()> {
        Ok(())
    }
    fn descriptor(&self) -> String {
        "wire transport".to_string()
    }
    /// The request id of the most recent response (`x-amzn-RequestId`), if known.
    fn last_request_id(&self) -> Option<String> {
        None
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&mut self, operation: &str, body: &str) -> Result<String> {
        (**self).send(operation, body)
    }
    fn on_connect(&mut self, region: &str, endpoint: &str) -> Result<()> {
        (**self).on_connect(region, endpoint)
    }
    fn descriptor(&self) -> String {
        (**self).descriptor()
    }
    fn last_request_id(&self) -> Option<String> {
        (**self).last_request_id()
    }
}

fn is_transient_wire_error(name: &str) -> bool {
    matches!(
        name,
        "ThrottlingException"
            | "ProvisionedThroughputExceededException"
            | "RequestLimitExceeded"
            | "InternalServerError"
            | "InternalServerErrorException"
            | "ServiceUnavailable"
            | "ServiceUnavailableException"
    )
}

fn compose_wire_error(prefix: &str, sep: &str, e: &Error) -> Error {
    let name = e.service_error_name.clone().unwrap_or_default();
    let reqid = e.request_id.clone().unwrap_or_default();
    Error {
        code: Errc::ServiceError,
        message: format!("{}{}. {}{}{}", prefix, name, reqid, sep, e.message),
        service_error_name: e.service_error_name.clone(),
        request_id: e.request_id.clone(),
    }
}

/// Small xorshift PRNG for backoff jitter (no external dependency).
fn seed() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    (t ^ (std::process::id() as u64).rotate_left(32)) | 1
}

pub struct WireClient<T: Transport> {
    transport: T,
    region: String,
    endpoint: String,
    /// Total attempts for a transient-error request (1 + retries).
    pub max_retry_attempts: u32,
    /// Sleep hook (tests replace it to avoid real sleeping).
    pub backoff_sleep: Box<dyn FnMut(Duration) + Send>,
    /// Told about each retry, after the backoff is computed and before it is
    /// slept (`--show-retries`). The waiters' polls are not retries.
    retry_observer: Option<RetryObserver>,
    rng: u64,
}

impl<T: Transport> WireClient<T> {
    pub fn new(transport: T) -> WireClient<T> {
        WireClient {
            transport,
            region: String::new(),
            endpoint: String::new(),
            max_retry_attempts: 11,
            backoff_sleep: Box::new(std::thread::sleep),
            retry_observer: None,
            rng: seed(),
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn jitter_unit(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The full-jitter backoff for the 0-based attempt: a uniform draw in
    /// [0, min(1000ms, 50ms * 2^attempt)].
    fn retry_delay(&mut self, attempt: u32) -> Duration {
        const BASE_MS: u64 = 50;
        const CAP_MS: u64 = 1000;
        let ceiling = if attempt < 20 {
            (BASE_MS << attempt).min(CAP_MS)
        } else {
            CAP_MS
        };
        let ms = (self.jitter_unit() * ceiling as f64) as u64;
        Duration::from_millis(ms)
    }

    fn send_retrying(&mut self, req: &WireRequest) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            let resp = self.transport.send(&req.operation, &req.body)?;
            let name = parse::error_type_name(&resp).filter(|n| is_transient_wire_error(n));
            let Some(name) = name else {
                return Ok(resp);
            };
            if attempt + 1 >= self.max_retry_attempts {
                return Ok(resp);
            }
            // The retry is decided: compute the backoff, tell the observer with
            // the delay about to be slept, then sleep and re-send.
            let delay = self.retry_delay(attempt);
            if let Some(observer) = self.retry_observer.as_mut() {
                observer(&RetryEvent {
                    operation: req.operation.clone(),
                    attempt: attempt + 2,
                    max_attempts: self.max_retry_attempts,
                    throttled: is_throttling_error(&name),
                    error_code: name,
                    delay: Some(delay),
                });
            }
            (self.backoff_sleep)(delay);
            attempt += 1;
        }
    }

    /// Attach the transport's request id to a service error that lacks one.
    fn with_request_id(&self, mut e: Error) -> Error {
        if e.service_error_name.is_some() && e.request_id.is_none() {
            e.request_id = self.transport.last_request_id();
        }
        e
    }

    fn round_trip<O>(&mut self, req: WireRequest, parse: fn(&str) -> Result<O>) -> Result<O> {
        let body = self.send_retrying(&req)?;
        parse(&body).map_err(|e| self.with_request_id(e))
    }

    fn round_trip_composed<O>(
        &mut self,
        req: WireRequest,
        parse: fn(&str) -> Result<O>,
        prefix: &str,
        sep: &str,
    ) -> Result<O> {
        let body = self.send_retrying(&req)?;
        parse(&body).map_err(|e| compose_wire_error(prefix, sep, &self.with_request_id(e)))
    }

    fn describe_for_wait(&mut self, table_name: &str) -> Result<Result<DescribeTableOut>> {
        let req = codec::serialize_describe_table(&DescribeTableIn {
            table_name: table_name.to_string(),
        });
        let body = self.transport.send(&req.operation, &req.body)?;
        Ok(parse::parse_describe_table(&body).map_err(|e| self.with_request_id(e)))
    }
}

impl<T: Transport> DdbClient for WireClient<T> {
    fn set_retry_observer(&mut self, observer: Option<RetryObserver>) {
        self.retry_observer = observer;
    }
    fn connect(&mut self, region: &str, endpoint: &str) -> Result<()> {
        self.region = region.to_string();
        self.endpoint = endpoint.to_string();
        self.transport.on_connect(region, endpoint)
    }

    fn connection_descriptor(&self) -> String {
        self.transport.descriptor()
    }

    fn get_item(&mut self, i: &GetItemIn) -> Result<GetItemOut> {
        self.round_trip(codec::serialize_get_item(i), parse::parse_get_item)
    }
    fn query(&mut self, i: &QueryIn) -> Result<QueryOut> {
        self.round_trip(codec::serialize_query(i), parse::parse_query)
    }
    fn scan(&mut self, i: &ScanIn) -> Result<ScanOut> {
        self.round_trip(codec::serialize_scan(i), parse::parse_scan)
    }
    fn search_vectors(&mut self, i: &SearchVectorsIn) -> Result<SearchVectorsOut> {
        self.round_trip(
            codec::serialize_search_vectors(i),
            parse::parse_search_vectors,
        )
    }
    fn put_item(&mut self, i: &PutItemIn) -> Result<PutItemOut> {
        self.round_trip(codec::serialize_put_item(i), parse::parse_put_item)
    }
    fn update_item(&mut self, i: &UpdateItemIn) -> Result<UpdateItemOut> {
        self.round_trip(codec::serialize_update_item(i), parse::parse_update_item)
    }
    fn delete_item(&mut self, i: &DeleteItemIn) -> Result<DeleteItemOut> {
        self.round_trip(codec::serialize_delete_item(i), parse::parse_delete_item)
    }

    fn explain_request(&mut self, request: &ExplainRequest) -> Result<String> {
        if let ExplainRequest::RestoreToPitr(i) = request {
            if iso8601_to_epoch(&i.restore_datetime).is_none() {
                return fail(
                    Errc::InvalidArgument,
                    format!("Invalid restore timestamp: {}", i.restore_datetime),
                );
            }
        }
        Ok(codec::serialize(request).body)
    }

    fn create_table(&mut self, i: &CreateTableIn) -> Result<()> {
        self.round_trip(codec::serialize_create_table(i), parse::parse_ack)
    }
    fn update_table(&mut self, i: &UpdateTableIn) -> Result<()> {
        self.round_trip_composed(
            codec::serialize_update_table(i),
            parse::parse_ack,
            "Update Table failed. ",
            ". ",
        )
    }
    fn update_time_to_live(&mut self, i: &UpdateTimeToLiveIn) -> Result<()> {
        self.round_trip_composed(
            codec::serialize_update_time_to_live(i),
            parse::parse_ack,
            "Error updating TTL. ",
            ".\n\t",
        )
    }
    fn update_continuous_backups(&mut self, i: &UpdateContinuousBackupsIn) -> Result<()> {
        self.round_trip_composed(
            codec::serialize_update_continuous_backups(i),
            parse::parse_ack,
            "Setting PITR failed. ",
            ".\n\t",
        )
    }
    fn create_backup(&mut self, i: &CreateBackupIn) -> Result<()> {
        self.round_trip(codec::serialize_create_backup(i), parse::parse_ack)
    }
    fn delete_backup(&mut self, i: &DeleteBackupIn) -> Result<()> {
        self.round_trip(codec::serialize_delete_backup(i), parse::parse_ack)
    }
    fn describe_backup(&mut self, i: &DescribeBackupIn) -> Result<DescribeBackupOut> {
        self.round_trip(
            codec::serialize_describe_backup(i),
            parse::parse_describe_backup,
        )
    }
    fn restore_table_from_backup(&mut self, i: &RestoreFromBackupIn) -> Result<()> {
        self.round_trip(codec::serialize_restore_from_backup(i), parse::parse_ack)
    }
    fn restore_table_to_point_in_time(&mut self, i: &RestoreToPitrIn) -> Result<()> {
        if iso8601_to_epoch(&i.restore_datetime).is_none() {
            return fail(
                Errc::InvalidArgument,
                format!("Invalid restore timestamp: {}", i.restore_datetime),
            );
        }
        self.round_trip(codec::serialize_restore_to_pitr(i), parse::parse_ack)
    }
    fn describe_limits(&mut self, i: &DescribeLimitsIn) -> Result<DescribeLimitsOut> {
        self.round_trip(
            codec::serialize_describe_limits(i),
            parse::parse_describe_limits,
        )
    }
    fn list_backups(&mut self, i: &ListBackupsIn) -> Result<ListBackupsOut> {
        self.round_trip(codec::serialize_list_backups(i), parse::parse_list_backups)
    }
    fn delete_table(&mut self, i: &DeleteTableIn) -> Result<()> {
        self.round_trip(codec::serialize_delete_table(i), parse::parse_ack)
    }
    fn list_tables(&mut self, _i: &ListTablesIn) -> Result<ListTablesOut> {
        let mut out = ListTablesOut::default();
        let mut page_in = ListTablesPageIn::default();
        loop {
            let page = self.list_tables_page(&page_in)?;
            out.table_names.extend(page.table_names);
            page_in.exclusive_start_table_name = page.last_evaluated_table_name;
            if page_in.exclusive_start_table_name.is_none() {
                return Ok(out);
            }
        }
    }
    fn list_tables_page(&mut self, i: &ListTablesPageIn) -> Result<ListTablesPageOut> {
        // An empty cursor means the first page; it is not sent.
        let cursor = i
            .exclusive_start_table_name
            .as_deref()
            .filter(|c| !c.is_empty());
        let req = codec::serialize_list_tables(cursor);
        let page = self.round_trip(req, parse::parse_list_tables)?;
        Ok(ListTablesPageOut {
            table_names: page.table_names,
            last_evaluated_table_name: page.last_evaluated_table_name,
        })
    }
    fn describe_table(&mut self, i: &DescribeTableIn) -> Result<DescribeTableOut> {
        self.round_trip(
            codec::serialize_describe_table(i),
            parse::parse_describe_table,
        )
    }
    fn transact_get_items(&mut self, i: &TransactGetItemsIn) -> Result<TransactGetItemsOut> {
        self.round_trip_composed(
            codec::serialize_transact_get_items(i),
            parse::parse_transact_get_items,
            "Transaction failed. ",
            ". ",
        )
    }
    fn transact_write_items(&mut self, i: &TransactWriteItemsIn) -> Result<()> {
        self.round_trip_composed(
            codec::serialize_transact_write_items(i),
            parse::parse_ack,
            "Transaction failed. ",
            ". ",
        )
    }
    fn describe_time_to_live(&mut self, i: &DescribeTimeToLiveIn) -> Result<TimeToLiveOut> {
        self.round_trip(
            codec::serialize_describe_time_to_live(i),
            parse::parse_describe_time_to_live,
        )
    }
    fn describe_continuous_backups(
        &mut self,
        i: &DescribeContinuousBackupsIn,
    ) -> Result<ContinuousBackupsOut> {
        self.round_trip(
            codec::serialize_describe_continuous_backups(i),
            parse::parse_describe_continuous_backups,
        )
    }
    fn describe_table_replica_auto_scaling(
        &mut self,
        i: &DescribeTableReplicaAutoScalingIn,
    ) -> Result<TableReplicaAutoScalingOut> {
        self.round_trip(
            codec::serialize_describe_table_replica_auto_scaling(i),
            parse::parse_describe_table_replica_auto_scaling,
        )
    }

    fn wait_for_table_active(&mut self, table_name: &str) -> Result<()> {
        let mut interval = Duration::from_millis(100);
        let cap = Duration::from_millis(2000);
        loop {
            match self.describe_for_wait(table_name)? {
                Ok(desc) => {
                    let status = desc.table_status;
                    if status == "ACTIVE" {
                        return Ok(());
                    }
                    if status != "CREATING" && status != "UPDATING" {
                        return fail(
                            Errc::ServiceError,
                            format!(
                                "Table {} is in state {} and will not become ACTIVE.",
                                table_name, status
                            ),
                        );
                    }
                }
                Err(e) => {
                    let name = e.service_error_name.clone().unwrap_or_default();
                    if name != "ResourceNotFoundException" && !is_transient_wire_error(&name) {
                        return Err(e);
                    }
                }
            }
            (self.backoff_sleep)(interval);
            interval = (interval * 2).min(cap);
        }
    }

    fn wait_for_table_gone(&mut self, table_name: &str) -> Result<()> {
        let mut interval = Duration::from_millis(100);
        let cap = Duration::from_millis(2000);
        loop {
            if let Err(e) = self.describe_for_wait(table_name)? {
                let name = e.service_error_name.clone().unwrap_or_default();
                if name == "ResourceNotFoundException" {
                    return Ok(());
                }
                if !is_transient_wire_error(&name) {
                    return Err(e);
                }
            }
            (self.backoff_sleep)(interval);
            interval = (interval * 2).min(cap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use std::collections::VecDeque;

    /// A transport that replays canned responses and records calls.
    #[derive(Default)]
    struct Canned {
        responses: VecDeque<Result<String>>,
        calls: Vec<(String, String)>,
        connects: Vec<(String, String)>,
    }
    impl Canned {
        fn with(responses: Vec<Result<String>>) -> Canned {
            Canned {
                responses: responses.into(),
                ..Default::default()
            }
        }
    }
    impl Transport for Canned {
        fn send(&mut self, op: &str, body: &str) -> Result<String> {
            self.calls.push((op.into(), body.into()));
            self.responses
                .pop_front()
                .unwrap_or_else(|| Ok("{}".into()))
        }
        fn on_connect(&mut self, region: &str, endpoint: &str) -> Result<()> {
            self.connects.push((region.into(), endpoint.into()));
            if region == "bad" {
                return fail(Errc::ServiceError, "transport rejected connect");
            }
            Ok(())
        }
        fn descriptor(&self) -> String {
            "wire/test -> canned backend".into()
        }
        fn last_request_id(&self) -> Option<String> {
            Some("REQ1".into())
        }
    }

    fn client(responses: Vec<Result<String>>) -> WireClient<Canned> {
        let mut c = WireClient::new(Canned::with(responses));
        c.backoff_sleep = Box::new(|_| {});
        c
    }

    const THROTTLE: &str = r#"{"__type":"com.amazonaws.dynamodb.v20120810#ThrottlingException","message":"slow down"}"#;

    #[test]
    fn get_item_round_trip() {
        let mut c = client(vec![Ok(
            r#"{"Item":{"id":{"S":"k1"},"v":{"N":"7"}}}"#.into()
        )]);
        let i = GetItemIn {
            table_name: "t".into(),
            key: vec![("id".into(), Value::S("k1".into()))],
            ..Default::default()
        };
        let out = c.get_item(&i).unwrap();
        assert_eq!(out.item.unwrap().attributes.len(), 2);
        assert_eq!(c.transport().calls[0].0, "GetItem");
        assert_eq!(c.transport().calls[0].1, codec::serialize_get_item(&i).body);
    }

    #[test]
    fn conditional_check_failed_is_mapped() {
        let mut c = client(vec![Ok(
            r#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"The conditional request failed"}"#.into(),
        )]);
        let e = c
            .put_item(&PutItemIn {
                table_name: "t".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(e.code, Errc::ConditionalCheckFailed);
        assert_eq!(e.message, "The conditional request failed");
        assert_eq!(e.request_id.as_deref(), Some("REQ1"));
    }

    #[test]
    fn transport_failure_surfaced_unchanged() {
        let mut c = client(vec![fail(Errc::ServiceError, "backend unreachable")]);
        let e = c.scan(&ScanIn::default()).unwrap_err();
        assert_eq!(e.message, "backend unreachable");
    }

    #[test]
    fn retries_transient_then_succeeds() {
        let mut c = client(vec![
            Ok(THROTTLE.into()),
            Ok(THROTTLE.into()),
            Ok(r#"{"Items":[]}"#.into()),
        ]);
        assert!(c.scan(&ScanIn::default()).is_ok());
        assert_eq!(c.transport().calls.len(), 3);
    }

    #[test]
    fn retry_observer_sees_each_retry_before_its_sleep() {
        use std::sync::{Arc, Mutex};
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut c = client(vec![
            Ok(THROTTLE.into()),
            Ok(r#"{"__type":"x#InternalServerError","message":"oops"}"#.into()),
            Ok(r#"{"Items":[]}"#.into()),
        ]);
        let l = log.clone();
        c.backoff_sleep = Box::new(move |d| l.lock().unwrap().push(format!("sleep {:?}", d)));
        let l = log.clone();
        c.set_retry_observer(Some(Box::new(move |e: &RetryEvent| {
            l.lock().unwrap().push(format!(
                "{} {} {}/{} {} {:?}",
                e.operation, e.error_code, e.attempt, e.max_attempts, e.throttled, e.delay
            ))
        })));
        assert!(c.scan(&ScanIn::default()).is_ok());
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 4);
        assert!(log[0].starts_with("Scan ThrottlingException 2/11 true Some("));
        assert!(log[1].starts_with("sleep "));
        assert!(log[2].starts_with("Scan InternalServerError 3/11 false Some("));
        // The reported delay is the slept delay.
        let delay = |s: &str| {
            s.rsplit("Some(")
                .next()
                .unwrap()
                .trim_end_matches(')')
                .to_string()
        };
        assert_eq!(delay(&log[0]), log[1].trim_start_matches("sleep "));
    }

    #[test]
    fn retry_budget_exhausted_returns_last_error() {
        let mut c = client((0..20).map(|_| Ok(THROTTLE.to_string())).collect());
        let e = c.scan(&ScanIn::default()).unwrap_err();
        assert_eq!(e.service_error_name.as_deref(), Some("ThrottlingException"));
        assert_eq!(c.transport().calls.len(), 11);
    }

    #[test]
    fn non_transient_not_retried() {
        let mut c = client(vec![Ok(
            r#"{"__type":"x#ValidationException","message":"bad"}"#.into(),
        )]);
        assert!(c.scan(&ScanIn::default()).is_err());
        assert_eq!(c.transport().calls.len(), 1);
    }

    #[test]
    fn composed_errors() {
        let mut c = client(vec![Ok(
            r#"{"__type":"x#TransactionCanceledException","message":"Transaction cancelled"}"#
                .into(),
        )]);
        let e = c
            .transact_write_items(&TransactWriteItemsIn::default())
            .unwrap_err();
        assert_eq!(
            e.message,
            "Transaction failed. TransactionCanceledException. REQ1. Transaction cancelled"
        );
        let mut c = client(vec![Ok(
            r#"{"__type":"x#ValidationException","message":"m"}"#.into(),
        )]);
        let e = c
            .update_time_to_live(&UpdateTimeToLiveIn::default())
            .unwrap_err();
        assert_eq!(
            e.message,
            "Error updating TTL. ValidationException. REQ1.\n\tm"
        );
    }

    #[test]
    fn explain_and_bad_timestamp() {
        let mut c = client(vec![]);
        let dt = DescribeTableIn {
            table_name: "t".into(),
        };
        assert_eq!(
            c.explain_request(&ExplainRequest::DescribeTable(dt.clone()))
                .unwrap(),
            codec::serialize_describe_table(&dt).body
        );
        let r = RestoreToPitrIn {
            restore_datetime: "not-a-timestamp".into(),
            ..Default::default()
        };
        let e = c
            .explain_request(&ExplainRequest::RestoreToPitr(r.clone()))
            .unwrap_err();
        assert_eq!(e.code, Errc::InvalidArgument);
        assert_eq!(e.message, "Invalid restore timestamp: not-a-timestamp");
        assert!(c.restore_table_to_point_in_time(&r).is_err());
        assert!(c.transport().calls.is_empty());
    }

    #[test]
    fn connect_and_descriptor() {
        let mut c = client(vec![]);
        c.connect("eu-west-1", "https://example.test:18443")
            .unwrap();
        assert_eq!(
            c.transport().connects[0],
            ("eu-west-1".into(), "https://example.test:18443".into())
        );
        assert_eq!(
            c.connect("bad", "").unwrap_err().message,
            "transport rejected connect"
        );
        assert_eq!(c.connection_descriptor(), "wire/test -> canned backend");
    }

    #[test]
    fn list_tables_paginates() {
        let mut c = client(vec![
            Ok(r#"{"TableNames":["a"],"LastEvaluatedTableName":"a"}"#.into()),
            Ok(r#"{"TableNames":["b"]}"#.into()),
        ]);
        assert_eq!(
            c.list_tables(&ListTablesIn {}).unwrap().table_names,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(c.transport().calls[0].1, "{\n}\n");
        assert!(c.transport().calls[1].1.contains("ExclusiveStartTableName"));
    }

    #[test]
    fn waiters() {
        let nf =
            r#"{"__type":"x#ResourceNotFoundException","message":"Requested resource not found"}"#;
        let mut c = client(vec![
            Ok(nf.into()),
            Ok(r#"{"Table":{"TableStatus":"CREATING"}}"#.into()),
            Ok(r#"{"Table":{"TableStatus":"ACTIVE"}}"#.into()),
        ]);
        c.wait_for_table_active("t").unwrap();
        assert_eq!(c.transport().calls.len(), 3);

        let mut c = client(vec![Ok(r#"{"Table":{"TableStatus":"DELETING"}}"#.into())]);
        let e = c.wait_for_table_active("t").unwrap_err();
        assert_eq!(
            e.message,
            "Table t is in state DELETING and will not become ACTIVE."
        );

        let mut c = client(vec![
            Ok(r#"{"Table":{"TableStatus":"DELETING"}}"#.into()),
            Ok(nf.into()),
        ]);
        c.wait_for_table_gone("t").unwrap();
    }
}
