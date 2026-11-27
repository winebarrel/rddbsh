// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Page-at-a-time SELECT row cursor (port of execute/select_cursor.cpp).

use super::consumed_capacity_totalizer::ConsumedCapacityTotalizer;
use super::rate_limiter::RateLimiter;
use crate::client::*;
use crate::error::{fail, Errc, Result};
use crate::parser::ast::SelectCmd;
use crate::planner::{plan_select, SelectApi};
use crate::value::ValueMap;

pub struct SelectRowCursor {
    api: SelectApi,
    get_item: GetItemIn,
    query: QueryIn,
    /// The upper leg of a not-equal split: issued, from its first page, once
    /// `query` is exhausted, and only if the caller asks for the next page
    /// then. Unset otherwise, and once it has started.
    query_upper: Option<QueryIn>,
    scan: ScanIn,
    limiter: Option<RateLimiter>,
    totalizer: ConsumedCapacityTotalizer,
    report_capacity: bool,
    column_hint: Vec<String>,
    more: bool,
    /// A ratelimit sleep is owed before the next request: set after a page
    /// that has a successor, taken at the start of the next fetch, so the
    /// previous page is emitted before the sleep.
    throttle_pending: bool,
}

pub fn open_select_cursor(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    alias_prefix: &str,
) -> Result<SelectRowCursor> {
    let plan = plan_select(cmd, client, alias_prefix)?;
    if plan.api == SelectApi::SearchVectors {
        return fail(
            Errc::InternalError,
            "open_select_cursor does not handle a vector search; route to execute_select",
        );
    }
    let limiter = match cmd.ratelimit {
        Some(rl) if plan.api != SelectApi::GetItem => Some(RateLimiter::new(rl.rcu, rl.wcu)),
        _ => None,
    };
    Ok(SelectRowCursor {
        api: plan.api,
        get_item: plan.get_item,
        query: plan.query,
        query_upper: plan.query_upper,
        scan: plan.scan,
        limiter,
        totalizer: ConsumedCapacityTotalizer::default(),
        report_capacity: cmd.rcc != ReturnConsumedCapacity::None,
        column_hint: plan.column_hint,
        more: true,
        throttle_pending: false,
    })
}

impl SelectRowCursor {
    /// The next non-empty page of rows, or `None` when exhausted.
    pub fn next_page(&mut self, client: &mut dyn DdbClient) -> Result<Option<Vec<ItemRow>>> {
        while self.more {
            if self.throttle_pending {
                self.throttle_pending = false;
                if let Some(l) = &mut self.limiter {
                    l.readlimit();
                }
            }
            let mut items: Vec<ItemRow> = Vec::new();
            let cc: Option<ConsumedCapacity>;
            let mut last_evaluated_key: Option<ValueMap> = None;
            match self.api {
                SelectApi::GetItem => {
                    let result = client.get_item(&self.get_item)?;
                    if let Some(item) = result.item {
                        items.push(item);
                    }
                    cc = result.consumed_capacity;
                    self.more = false;
                }
                SelectApi::Query => {
                    let result = client.query(&self.query)?;
                    items = result.items;
                    cc = result.consumed_capacity;
                    last_evaluated_key = result.last_evaluated_key;
                }
                SelectApi::Scan => {
                    let result = client.scan(&self.scan)?;
                    items = result.items;
                    cc = result.consumed_capacity;
                    last_evaluated_key = result.last_evaluated_key;
                }
                SelectApi::SearchVectors => {
                    cc = None;
                    self.more = false;
                }
            }
            self.totalizer.add(&cc);
            if let Some(l) = &mut self.limiter {
                l.consume_reads(&cc);
            }
            if matches!(self.api, SelectApi::Query | SelectApi::Scan) {
                match last_evaluated_key {
                    Some(lek) if !lek.is_empty() => {
                        if self.api == SelectApi::Query {
                            self.query.exclusive_start_key = Some(lek);
                        } else {
                            self.scan.exclusive_start_key = Some(lek);
                        }
                        self.throttle_pending = self.limiter.is_some();
                    }
                    _ => {
                        if let Some(upper) = self.query_upper.take() {
                            // The lower leg of a not-equal split is exhausted:
                            // the next fetch starts the upper leg from its
                            // first page, metered like any next page.
                            self.query = upper;
                            self.throttle_pending = self.limiter.is_some();
                        } else {
                            self.more = false;
                        }
                    }
                }
            }
            if !items.is_empty() {
                return Ok(Some(items));
            }
        }
        Ok(None)
    }

    /// The columns the output formats show first (SelectOutcome::column_hint).
    pub fn column_hint(&self) -> &[String] {
        &self.column_hint
    }

    pub fn consumed_capacity(&self) -> Option<ConsumedCapacity> {
        if self.report_capacity {
            self.totalizer.get()
        } else {
            None
        }
    }
}
