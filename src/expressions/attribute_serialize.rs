// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Compact `{"S":"x"}` style serialization used by the un-aliased EXPLAIN /
//! debug renderings (port of expressions/attribute_serialize.cpp).

use crate::value::Value;

fn append_escaped(out: &mut String, input: &str) {
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
}

fn serialize_impl(value: &Value) -> String {
    match value {
        Value::S(s) => {
            let mut out = String::from("{\"S\":\"");
            append_escaped(&mut out, s);
            out.push_str("\"}");
            out
        }
        Value::N(n) => format!("{{\"N\":\"{}\"}}", n),
        Value::B(b) => {
            let mut out = String::from("{\"B\":\"");
            append_escaped(&mut out, b);
            out.push_str("\"}");
            out
        }
        Value::Bool(b) => if *b {
            "{\"BOOL\":true}"
        } else {
            "{\"BOOL\":false}"
        }
        .to_string(),
        Value::Null | Value::Param(_) => "{\"NULL\":true}".to_string(),
        Value::L(list) => {
            let parts: Vec<String> = list.iter().map(serialize_impl).collect();
            format!("{{\"L\":[{}]}}", parts.join(","))
        }
        Value::M(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let mut out = String::from("\"");
                    append_escaped(&mut out, k);
                    out.push_str("\":");
                    out.push_str(&serialize_impl(v));
                    out
                })
                .collect();
            format!("{{\"M\":{{{}}}}}", parts.join(","))
        }
        Value::SS(set) | Value::NS(set) | Value::BS(set) => {
            let tag = match value {
                Value::SS(_) => "\"SS\"",
                Value::NS(_) => "\"NS\"",
                _ => "\"BS\"",
            };
            let parts: Vec<String> = set
                .iter()
                .map(|e| {
                    let mut out = String::from("\"");
                    append_escaped(&mut out, e);
                    out.push('"');
                    out
                })
                .collect();
            format!("{{{}:[{}]}}", tag, parts.join(","))
        }
    }
}

pub fn serialize_attribute(value: &Value) -> String {
    serialize_impl(value).replace('\n', "")
}
