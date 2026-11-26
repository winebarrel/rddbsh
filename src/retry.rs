// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Retry notification (port of include/ddbsh/retry_event.hpp, ddbsh 0.12.3).

use std::time::Duration;

/// One retry a transport has decided to make, raised before its backoff.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RetryEvent {
    /// The DynamoDB operation being retried ("Scan", ...); empty when unknown.
    pub operation: String,
    /// The attempt about to be made, counting the first send as 1.
    pub attempt: u32,
    /// The transport's attempt budget (first send included); 0 when unknown.
    pub max_attempts: u32,
    /// The error that caused the retry; empty when it carried no name.
    pub error_code: String,
    /// True when the error is a throttling error.
    pub throttled: bool,
    /// The backoff about to be slept before the retry.
    pub delay: Option<Duration>,
}

/// Receives RetryEvents synchronously, before the backoff sleep.
pub type RetryObserver = Box<dyn FnMut(&RetryEvent) + Send>;

/// True when `error_code` names a DynamoDB throttling error (the codes the AWS
/// SDK for C++ classifies as throttling). Exact, case-sensitive match.
pub fn is_throttling_error(error_code: &str) -> bool {
    matches!(
        error_code,
        "ThrottlingException"
            | "ProvisionedThroughputExceededException"
            | "RequestLimitExceeded"
            | "TooManyRequestsException"
            | "LimitExceededException"
            | "TransactionInProgressException"
    )
}
