// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Key-schema resolution and the SELECT GetItem/Query/Scan planner
//! (port of src/planner).
pub mod get_key_schema;
pub mod select_planner;

pub use get_key_schema::{get_key_schema, ResolvedKeySchema};
pub use select_planner::{plan_select, SelectApi, SelectPlan};
