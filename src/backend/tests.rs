//! Backend unit tests: fixture parsing + CLI command construction.

use super::cli::{CliBackend, port_flags};
use super::{CreateSpec, LogEntry};
use crate::models::PublishedPort;

#[test]
fn parse_logs_fixture() {
    let text = include_str!("../fixtures/logs.json");
    let lines = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str::<LogEntry>)
        .collect::<Result<Vec<_>, _>>()
        .expect("all log lines parse");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].s, "stdout");
    assert_eq!(lines[1].s, "stderr");
    assert_eq!(lines[0].d, "hello-stdout\n");
    assert!(lines[0].timestamp().is_ok());
}

#[test]
fn create_spec_to_args_minimal() {
    let spec = CreateSpec {
        image: "alpine".into(),
        ..Default::default()
    };
    assert_eq!(spec.to_args(), vec!["alpine"]);
}

#[test]
fn create_spec_to_args_full() {
    let spec = CreateSpec {
        image: "python".into(),
        name: Some("api".into()),
        cpus: Some(2),
        memory: Some("512M".into()),
        workdir: Some("/app".into()),
        ports: vec![PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: 8080,
            guest_port: 80,
            protocol: "tcp".into(),
        }],
        volumes: vec!["./src:/app".into()],
        env: vec!["DEBUG=true".into()],
        labels: vec!["app=demo".into()],
        net_profile: Some("public".into()),
        net_rules: vec!["allow@api.example.com".into()],
    };
    let args = spec.to_args();
    assert!(args.starts_with(&[
        "--name".to_string(),
        "api".to_string(),
        "-c".to_string(),
        "2".to_string(),
        "-m".to_string(),
        "512M".to_string(),
        "-w".to_string(),
        "/app".to_string(),
    ]));
    assert!(args.contains(&"-p".to_string()));
    assert!(args.contains(&"8080:80".to_string()));
    assert!(args.contains(&"-v".to_string()));
    assert!(args.contains(&"./src:/app".to_string()));
    assert!(args.contains(&"-e".to_string()));
    assert!(args.contains(&"DEBUG=true".to_string()));
    assert!(args.contains(&"--label".to_string()));
    assert!(args.contains(&"app=demo".to_string()));
    assert!(args.contains(&"--net".to_string()));
    assert!(args.contains(&"public".to_string()));
    assert!(args.contains(&"--net-rule".to_string()));
    assert!(args.contains(&"allow@api.example.com".to_string()));
    assert!(args.contains(&"python".to_string()));
}

#[test]
fn port_flags_roundtrip() {
    let ports = vec![
        PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: 8080,
            guest_port: 80,
            protocol: "tcp".into(),
        },
        PublishedPort {
            host_bind: "0.0.0.0".into(),
            host_port: 9090,
            guest_port: 90,
            protocol: "udp".into(),
        },
    ];
    let flags = port_flags(&ports);
    assert_eq!(flags, vec!["-p", "8080:80", "-p", "0.0.0.0:9090:90/udp"]);
}

#[test]
fn cli_backend_program_name() {
    let b = CliBackend::with_program("/usr/local/bin/msb");
    assert_eq!(b.program(), "/usr/local/bin/msb");
}
