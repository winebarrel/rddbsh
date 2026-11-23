// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The SigV4-signed HTTPS transport to DynamoDB (replaces the C++ AWS-SDK
//! adapter, aws/aws_ddb_client.cpp), plus the CA-bundle helpers.
//!
//! Credentials and (when not given) the region come from aws-config's default
//! chain: environment, shared config/credentials profiles (AWS_PROFILE),
//! SSO, credential_process, web identity, ECS/IMDS. A tokio runtime is used
//! only to drive that async chain; HTTP itself is blocking (ureq).

use std::time::{Duration, SystemTime};

use aws_config::BehaviorVersion;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;

use super::client::{Transport, WireClient};
use crate::client::DdbClient;
use crate::error::{Errc, Error, Result};

const TARGET_PREFIX: &str = "DynamoDB_20120810.";
const CONTENT_TYPE: &str = "application/x-amz-json-1.0";
const DEFAULT_REGION: &str = "us-east-1";

fn transport_error(message: impl Into<String>) -> Error {
    Error::new(Errc::ServiceError, message)
}

/// Resolve the TLS CA bundle with AWS-CLI precedence: the `--ca-bundle` flag,
/// then `AWS_CA_BUNDLE`, then the profile's `ca_bundle`. Empty values do not count.
pub fn resolve_ca_bundle(
    flag: Option<String>,
    env: Option<String>,
    profile: Option<String>,
) -> Option<String> {
    [flag, env, profile]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
}

/// The active profile name, resolved like the credentials:
/// AWS_PROFILE, else AWS_DEFAULT_PROFILE, else `default`.
fn active_profile(explicit: Option<&str>) -> String {
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    for var in ["AWS_PROFILE", "AWS_DEFAULT_PROFILE"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    "default".to_string()
}

fn config_file_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AWS_CONFIG_FILE") {
        if !p.is_empty() {
            return Some(p.into());
        }
    }
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(std::path::Path::new(&home).join(".aws").join("config"))
}

/// Read `key` from `[profile <name>]` (or `[default]` / `[profile default]`)
/// in shared-config text.
pub fn config_profile_value(text: &str, profile: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let mut found: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim();
            let name = name.strip_prefix("profile ").map(str::trim).unwrap_or(name);
            in_section = name == profile;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                found = Some(v.trim().to_string());
            }
        }
    }
    found.filter(|v| !v.is_empty())
}

/// The `ca_bundle` setting of the active profile in the shared config file.
pub fn profile_ca_bundle() -> Option<String> {
    let path = config_file_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    config_profile_value(&text, &active_profile(None), "ca_bundle")
}

/// Build the endpoint URL (without trailing slash) for a region/override.
pub fn endpoint_url(region: &str, endpoint: &str) -> String {
    if !endpoint.is_empty() {
        let e = if endpoint.contains("://") {
            endpoint.to_string()
        } else {
            format!("https://{}", endpoint)
        };
        return e.trim_end_matches('/').to_string();
    }
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    format!("https://dynamodb.{}.{}", region, suffix)
}

/// `host[:port]` of a URL, omitting the scheme's default port.
fn host_header(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority
        .rsplit_once('@')
        .map(|(_, a)| a)
        .unwrap_or(authority);
    let default_port = if scheme == "http" { ":80" } else { ":443" };
    authority
        .strip_suffix(default_port)
        .unwrap_or(authority)
        .to_string()
}

fn build_agent(ca_bundle: Option<&str>) -> Result<ureq::Agent> {
    let mut builder = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)));
    if let Some(path) = ca_bundle {
        let pem = std::fs::read(path)
            .map_err(|e| transport_error(format!("Unable to read CA bundle {}: {}", path, e)))?;
        let mut certs = Vec::new();
        for item in ureq::tls::parse_pem(&pem) {
            if let Ok(ureq::tls::PemItem::Certificate(c)) = item {
                certs.push(c);
            }
        }
        if certs.is_empty() {
            return Err(transport_error(format!(
                "No certificates found in CA bundle {}",
                path
            )));
        }
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::new_with_certs(&certs))
            .build();
        builder = builder.tls_config(tls);
    }
    Ok(builder.build().into())
}

/// The blocking SigV4 HTTPS transport.
pub struct HttpTransport {
    region: String,
    endpoint: String,
    url: String,
    ca_bundle: Option<String>,
    agent: ureq::Agent,
    runtime: Option<tokio::runtime::Runtime>,
    provider: Option<SharedCredentialsProvider>,
    cached: Option<Credentials>,
    last_request_id: Option<String>,
    user_agent: String,
}

impl HttpTransport {
    /// `region` empty => resolved from the aws-config chain (else us-east-1);
    /// `endpoint` empty => the regional DynamoDB endpoint.
    pub fn new(region: &str, endpoint: &str, ca_bundle: Option<String>) -> Result<HttpTransport> {
        let agent = build_agent(ca_bundle.as_deref())?;
        let mut t = HttpTransport {
            region: String::new(),
            endpoint: String::new(),
            url: String::new(),
            ca_bundle,
            agent,
            runtime: None,
            provider: None,
            cached: None,
            last_request_id: None,
            user_agent: format!("ddbsh - version {}", crate::version()),
        };
        t.retarget(region, endpoint);
        Ok(t)
    }

    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn ca_bundle(&self) -> Option<&str> {
        self.ca_bundle.as_deref()
    }

    fn runtime(&mut self) -> Result<&tokio::runtime::Runtime> {
        if self.runtime.is_none() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    transport_error(format!("Unable to start the credentials runtime: {}", e))
                })?;
            self.runtime = Some(rt);
        }
        Ok(self.runtime.as_ref().unwrap())
    }

    fn retarget(&mut self, region: &str, endpoint: &str) {
        let region = if region.is_empty() {
            self.resolve_default_region()
        } else {
            region.to_string()
        };
        self.url = endpoint_url(&region, endpoint);
        self.region = region;
        self.endpoint = endpoint.to_string();
        // Re-resolve credentials on the next request (a CONNECT may change account/region).
        self.provider = None;
        self.cached = None;
    }

    fn resolve_default_region(&mut self) -> String {
        for var in ["AWS_REGION", "AWS_DEFAULT_REGION"] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        let loaded = match self.runtime() {
            Ok(rt) => rt.block_on(aws_config::defaults(BehaviorVersion::latest()).load()),
            Err(_) => return DEFAULT_REGION.to_string(),
        };
        loaded
            .region()
            .map(|r| r.to_string())
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

    fn credentials(&mut self) -> Result<Credentials> {
        if let Some(c) = &self.cached {
            let fresh = match c.expiry() {
                None => true,
                Some(exp) => exp
                    .duration_since(SystemTime::now())
                    .map(|d| d > Duration::from_secs(300))
                    .unwrap_or(false),
            };
            if fresh {
                return Ok(c.clone());
            }
        }
        let region = self.region.clone();
        let provider = match self.provider.clone() {
            Some(p) => p,
            None => {
                let rt = self.runtime()?;
                let config = rt.block_on(
                    aws_config::defaults(BehaviorVersion::latest())
                        .region(aws_config::Region::new(region))
                        .load(),
                );
                let p = config
                    .credentials_provider()
                    .ok_or_else(|| transport_error("No AWS credentials provider is configured"))?;
                self.provider = Some(p.clone());
                p
            }
        };
        let rt = self.runtime()?;
        let creds = rt.block_on(provider.provide_credentials()).map_err(|e| {
            transport_error(format!(
                "Unable to load AWS credentials: {}",
                error_chain(&e)
            ))
        })?;
        self.cached = Some(creds.clone());
        Ok(creds)
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

impl Transport for HttpTransport {
    fn send(&mut self, operation: &str, body: &str) -> Result<String> {
        let creds = self.credentials()?;
        let identity: Identity = creds.into();
        let host = host_header(&self.url);
        let target = format!("{}{}", TARGET_PREFIX, operation);
        let url = format!("{}/", self.url);
        let headers: Vec<(&str, &str)> = vec![
            ("host", host.as_str()),
            ("content-type", CONTENT_TYPE),
            ("x-amz-target", target.as_str()),
        ];
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("dynamodb")
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .map_err(|e| transport_error(format!("Unable to sign request: {}", e)))?
            .into();
        let signable = SignableRequest::new(
            "POST",
            url.as_str(),
            headers.iter().copied(),
            SignableBody::Bytes(body.as_bytes()),
        )
        .map_err(|e| transport_error(format!("Unable to sign request: {}", e)))?;
        let (instructions, _sig) = sign(signable, &params)
            .map_err(|e| transport_error(format!("Unable to sign request: {}", e)))?
            .into_parts();

        let mut request = self.agent.post(&url).header("user-agent", &self.user_agent);
        for (k, v) in &headers {
            request = request.header(*k, *v);
        }
        for (k, v) in instructions.headers() {
            request = request.header(k, v);
        }
        let mut response = request
            .send(body)
            .map_err(|e| transport_error(e.to_string()))?;
        self.last_request_id = response
            .headers()
            .get("x-amzn-requestid")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| transport_error(e.to_string()))?;
        if status >= 400 && super::parse::error_type_name(&text).is_none() {
            // A non-JSON (or untyped) error page: synthesize a typed document so the
            // client maps it like any other service error.
            let name = if status >= 500 {
                "InternalServerError"
            } else {
                "HttpError"
            };
            let msg = if text.trim().is_empty() {
                format!("HTTP status {}", status)
            } else {
                text.trim().to_string()
            };
            return Ok(format!(
                "{{\"__type\":\"{}\",\"message\":{}}}",
                name,
                json_quote(&msg)
            ));
        }
        Ok(text)
    }

    fn on_connect(&mut self, region: &str, endpoint: &str) -> Result<()> {
        self.retarget(region, endpoint);
        Ok(())
    }

    fn descriptor(&self) -> String {
        "rust sigv4 wire (direct DynamoDB)".to_string()
    }

    fn last_request_id(&self) -> Option<String> {
        self.last_request_id.clone()
    }
}

fn json_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Open a DynamoDB client: SigV4 HTTPS transport targeting `endpoint` (empty =>
/// the regional endpoint) in `region` (empty => resolved from the AWS chain),
/// trusting `ca_bundle` (a PEM file) when given.
pub fn open(
    region: &str,
    endpoint: &str,
    ca_bundle: Option<String>,
) -> Result<WireClient<HttpTransport>> {
    let transport = HttpTransport::new(region, endpoint, ca_bundle)?;
    let resolved = transport.region().to_string();
    let mut client = WireClient::new(transport);
    // Record the target on the client without re-resolving anything.
    client.connect(&resolved, endpoint)?;
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::*;
    use crate::table_schema::*;
    use crate::value::Value;

    #[test]
    fn ca_bundle_precedence() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(resolve_ca_bundle(s("f"), s("e"), s("p")), s("f"));
        assert_eq!(resolve_ca_bundle(s(""), s("e"), s("p")), s("e"));
        assert_eq!(resolve_ca_bundle(None, s(""), s("p")), s("p"));
        assert_eq!(resolve_ca_bundle(None, None, s("")), None);
    }

    #[test]
    fn profile_value_parsing() {
        let text = "[default]\nregion = us-east-1\nca_bundle = /d.pem\n\n[profile dev]\nca_bundle=/dev.pem\n[profile empty]\nca_bundle =\n";
        assert_eq!(
            config_profile_value(text, "default", "ca_bundle").as_deref(),
            Some("/d.pem")
        );
        assert_eq!(
            config_profile_value(text, "dev", "ca_bundle").as_deref(),
            Some("/dev.pem")
        );
        assert_eq!(config_profile_value(text, "empty", "ca_bundle"), None);
        assert_eq!(config_profile_value(text, "missing", "ca_bundle"), None);
    }

    #[test]
    fn endpoints_and_hosts() {
        assert_eq!(
            endpoint_url("us-west-2", ""),
            "https://dynamodb.us-west-2.amazonaws.com"
        );
        assert_eq!(
            endpoint_url("cn-north-1", ""),
            "https://dynamodb.cn-north-1.amazonaws.com.cn"
        );
        assert_eq!(
            endpoint_url("x", "http://localhost:8000/"),
            "http://localhost:8000"
        );
        assert_eq!(
            endpoint_url("x", "example.test:18443"),
            "https://example.test:18443"
        );
        assert_eq!(host_header("http://localhost:8000"), "localhost:8000");
        assert_eq!(
            host_header("https://dynamodb.us-east-1.amazonaws.com"),
            "dynamodb.us-east-1.amazonaws.com"
        );
        assert_eq!(host_header("https://h.test:443"), "h.test");
    }

    #[test]
    fn missing_ca_bundle_file_is_an_error() {
        let e = HttpTransport::new("us-east-1", "", Some("/nonexistent/ca.pem".into()))
            .err()
            .unwrap();
        assert!(
            e.message.contains("Unable to read CA bundle"),
            "{}",
            e.message
        );
    }

    /// Live round trip against DynamoDB Local:
    /// `AWS_ACCESS_KEY_ID=x AWS_SECRET_ACCESS_KEY=x DDBSH_TEST_ENDPOINT=http://localhost:8000 cargo test --lib wire -- --ignored`
    #[test]
    #[ignore]
    fn live_dynamodb_local_round_trip() {
        let endpoint =
            std::env::var("DDBSH_TEST_ENDPOINT").unwrap_or_else(|_| "http://localhost:8000".into());
        let mut c = open("us-east-1", &endpoint, None).unwrap();
        let table = format!("rddbsh_wire_live_{}", std::process::id());
        c.create_table(&CreateTableIn {
            table_name: table.clone(),
            attribute_definitions: vec![AttributeDef {
                name: "a".into(),
                ty: ScalarType::N,
            }],
            key_schema: KeySchema {
                hash: vec!["a".into()],
                range: vec![],
            },
            ..Default::default()
        })
        .unwrap();
        c.wait_for_table_active(&table).unwrap();
        let dup = c
            .create_table(&CreateTableIn {
                table_name: table.clone(),
                attribute_definitions: vec![AttributeDef {
                    name: "a".into(),
                    ty: ScalarType::N,
                }],
                key_schema: KeySchema {
                    hash: vec!["a".into()],
                    range: vec![],
                },
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(
            dup.service_error_name.as_deref(),
            Some("ResourceInUseException")
        );
        assert!(c
            .list_tables(&ListTablesIn {})
            .unwrap()
            .table_names
            .contains(&table));
        c.put_item(&PutItemIn {
            table_name: table.clone(),
            item: vec![
                ("a".into(), Value::N("1".into())),
                ("b".into(), Value::S("x\u{e9}".into())),
            ],
            ..Default::default()
        })
        .unwrap();
        let ccf = c
            .put_item(&PutItemIn {
                table_name: table.clone(),
                item: vec![("a".into(), Value::N("1".into()))],
                condition_expression: Some("attribute_not_exists(#k)".into()),
                expression_attribute_names: vec![("#k".into(), "a".into())],
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(ccf.code, Errc::ConditionalCheckFailed);
        let got = c
            .get_item(&GetItemIn {
                table_name: table.clone(),
                key: vec![("a".into(), Value::N("1".into()))],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(got.item.unwrap().attributes.len(), 2);
        let scan = c
            .scan(&ScanIn {
                table_name: table.clone(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(scan.items.len(), 1);
        let d = c
            .describe_table(&DescribeTableIn {
                table_name: table.clone(),
            })
            .unwrap();
        assert_eq!(d.table_status, "ACTIVE");
        c.delete_table(&DeleteTableIn {
            table_name: table.clone(),
        })
        .unwrap();
        c.wait_for_table_gone(&table).unwrap();
        let nf = c
            .describe_table(&DescribeTableIn { table_name: table })
            .unwrap_err();
        assert_eq!(
            nf.service_error_name.as_deref(),
            Some("ResourceNotFoundException")
        );
    }
}
