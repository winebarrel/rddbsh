// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Sums ConsumedCapacity across pages/requests
//! (port of execute/consumed_capacity_totalizer.hpp).

use crate::client::ConsumedCapacity;

#[derive(Debug, Clone, Default)]
pub struct ConsumedCapacityTotalizer {
    seen: bool,
    total: ConsumedCapacity,
}

fn accumulate(sink: &mut Option<f64>, v: Option<f64>) {
    if let Some(v) = v {
        *sink = Some(sink.unwrap_or(0.0) + v);
    }
}

impl ConsumedCapacityTotalizer {
    pub fn add(&mut self, cc: &Option<ConsumedCapacity>) {
        let Some(cc) = cc else { return };
        self.seen = true;
        if self.total.table_name.is_none() && cc.table_name.is_some() {
            self.total.table_name = cc.table_name.clone();
        }
        accumulate(&mut self.total.capacity_units, cc.capacity_units);
        accumulate(&mut self.total.read_capacity_units, cc.read_capacity_units);
        accumulate(
            &mut self.total.write_capacity_units,
            cc.write_capacity_units,
        );
    }

    pub fn get(&self) -> Option<ConsumedCapacity> {
        if self.seen {
            Some(self.total.clone())
        } else {
            None
        }
    }
}
