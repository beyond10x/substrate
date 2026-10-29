//! Design 34 § 7.2: the file-operation request and response framing, and § 7.1's handshake.

use std::io::Cursor;

use substrate_executor::Handshake;
use substrate_executor::message::{
    ErrorBody, ErrorClass, MAX_CONTENT_BYTES, MAX_HEADER_BYTES, MessageError, PROTOCOL_FAILURE,
    Request, Response, read_message, read_request, read_response, write_message,
};
use substrate_guarded::{DriverError, DriverErrorClass};
use substrate_wire::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileMode, FilePatchInput,
    FileReadQuery, FileReplaceInput, LinePatchEdit, TextMatchPolicy, WorkspaceTreeQuery,
};

fn file_query() -> FileReadQuery {
    FileReadQuery {
        mode: FileMode::File,
        offset: Some(0),
        limit_bytes: Some(64),
        cursor: None,
        limit_items: None,
    }
}

fn every_request() -> Vec<(Request, Vec<u8>)> {
    let workspace = "ws_01".to_owned();
    let path = "dir/a $(HOME).txt".to_owned();
    vec![
        (
            Request::Read {
                workspace: workspace.clone(),
                path: path.clone(),
                query: file_query(),
            },
            Vec::new(),
        ),
        (
            Request::Write {
                workspace: workspace.clone(),
                path: path.clone(),
            },
            b"bytes \0 with a NUL".to_vec(),
        ),
        (
            Request::Delete {
                workspace: workspace.clone(),
                path: path.clone(),
            },
            Vec::new(),
        ),
        (
            Request::ReadV2 {
                workspace: workspace.clone(),
                path: path.clone(),
                query: file_query(),
            },
            Vec::new(),
        ),
        (
            Request::TreeV2 {
                workspace: workspace.clone(),
                query: WorkspaceTreeQuery {
                    limit_items: 10,
                    include_hidden: false,
                },
            },
            Vec::new(),
        ),
        (
            Request::ReplaceV2 {
                workspace: workspace.clone(),
                path: path.clone(),
                input: FileReplaceInput {
                    expected: ExpectedFileState::Absent,
                    content: Base64Content {
                        encoding: Base64Encoding::Base64,
                        data: "aGk=".to_owned(),
                    },
                    create_parents: true,
                },
            },
            Vec::new(),
        ),
        (
            Request::EditV2 {
                workspace: workspace.clone(),
                path: path.clone(),
                input: FileEditInput {
                    expected_sha256: "0".repeat(64),
                    old_text: "a".to_owned(),
                    new_text: "b".to_owned(),
                    match_policy: TextMatchPolicy::Exact,
                },
            },
            Vec::new(),
        ),
        (
            Request::PatchV2 {
                workspace: workspace.clone(),
                path: path.clone(),
                input: FilePatchInput {
                    expected_sha256: "0".repeat(64),
                    edits: vec![LinePatchEdit::InsertBefore {
                        line: 1,
                        text: "x".to_owned(),
                    }],
                },
            },
            Vec::new(),
        ),
        (Request::Stat { workspace, path }, Vec::new()),
        (Request::Check {}, Vec::new()),
    ]
}

#[test]
fn every_request_header_round_trips_with_its_content() {
    for (request, content) in every_request() {
        let mut wire = Vec::new();
        request.write_to(&mut wire, &content).expect("encode");
        let (decoded, decoded_content) = read_request(&mut Cursor::new(&wire)).expect("decode");
        assert_eq!(decoded, request);
        assert_eq!(decoded_content, content);
    }
}

#[test]
fn the_framing_is_a_u32be_header_then_a_u64be_content_length() {
    let mut wire = Vec::new();
    write_message(&mut wire, br#"{"op":"check"}"#, b"xyz").expect("encode");
    let mut expected = Vec::new();
    expected.extend_from_slice(&14_u32.to_be_bytes());
    expected.extend_from_slice(br#"{"op":"check"}"#);
    expected.extend_from_slice(&3_u64.to_be_bytes());
    expected.extend_from_slice(b"xyz");
    assert_eq!(wire, expected);
    let message = read_message(&mut Cursor::new(&wire)).expect("decode");
    assert_eq!(message.header, br#"{"op":"check"}"#);
    assert_eq!(message.content, b"xyz");
}

#[test]
fn a_header_or_content_over_its_bound_and_trailing_bytes_are_refused() {
    let mut oversized = Vec::new();
    oversized.extend_from_slice(&(MAX_HEADER_BYTES + 1).to_be_bytes());
    assert!(matches!(
        read_message(&mut Cursor::new(&oversized)),
        Err(MessageError::HeaderTooLarge(_))
    ));

    let mut content = Vec::new();
    content.extend_from_slice(&2_u32.to_be_bytes());
    content.extend_from_slice(b"{}");
    content.extend_from_slice(&(MAX_CONTENT_BYTES + 1).to_be_bytes());
    assert!(matches!(
        read_message(&mut Cursor::new(&content)),
        Err(MessageError::ContentTooLarge(_))
    ));

    let mut trailing = Vec::new();
    write_message(&mut trailing, br#"{"op":"check"}"#, b"").expect("encode");
    trailing.push(0);
    assert!(matches!(
        read_message(&mut Cursor::new(&trailing)),
        Err(MessageError::Trailing)
    ));

    let mut short = Vec::new();
    write_message(&mut short, br#"{"op":"check"}"#, b"abc").expect("encode");
    short.pop();
    assert!(matches!(
        read_message(&mut Cursor::new(&short)),
        Err(MessageError::Io(_))
    ));
}

#[test]
fn an_unknown_op_member_or_content_on_a_contentless_op_is_refused() {
    for header in [
        r#"{"op":"exec","argv":["sh"]}"#,
        r#"{"op":"check","namespace":"other"}"#,
        r#"{"op":"delete","workspace":"ws_01","path":"a","node":"n"}"#,
    ] {
        let mut wire = Vec::new();
        write_message(&mut wire, header.as_bytes(), b"").expect("encode");
        assert!(
            matches!(
                read_request(&mut Cursor::new(&wire)),
                Err(MessageError::Json(_))
            ),
            "{header}"
        );
    }
    let mut wire = Vec::new();
    write_message(
        &mut wire,
        br#"{"op":"delete","workspace":"ws_01","path":"a"}"#,
        b"x",
    )
    .expect("encode");
    assert!(matches!(
        read_request(&mut Cursor::new(&wire)),
        Err(MessageError::UnexpectedContent)
    ));
}

#[test]
fn ok_and_error_responses_round_trip() {
    let ok = Response::Ok {
        value: serde_json::json!({"kind": "file", "size": 3}),
        omitted_entries: 2,
    };
    let mut wire = Vec::new();
    ok.write_to(&mut wire).expect("encode");
    let header = read_message(&mut Cursor::new(&wire)).expect("frame").header;
    assert_eq!(
        String::from_utf8(header).expect("utf-8"),
        r#"{"ok":{"kind":"file","size":3},"omitted_entries":2}"#
    );
    assert_eq!(read_response(&mut Cursor::new(&wire)).expect("decode"), ok);

    let complete = Response::Ok {
        value: serde_json::json!({"absent": true}),
        omitted_entries: 0,
    };
    let mut wire = Vec::new();
    complete.write_to(&mut wire).expect("encode");
    let header = read_message(&mut Cursor::new(&wire)).expect("frame").header;
    assert_eq!(
        String::from_utf8(header).expect("utf-8"),
        r#"{"ok":{"absent":true}}"#
    );

    let error = Response::Error(ErrorBody::from(&DriverError::refused(
        "workspace.path-escape",
        "Workspace path is outside the confined root.",
        "path",
    )));
    let mut wire = Vec::new();
    error.write_to(&mut wire).expect("encode");
    let header = read_message(&mut Cursor::new(&wire)).expect("frame").header;
    assert_eq!(
        String::from_utf8(header).expect("utf-8"),
        r#"{"error":{"class":"refused","code":"workspace.path-escape","message":"Workspace path is outside the confined root.","address":"path","retriable":false}}"#
    );
    assert_eq!(
        read_response(&mut Cursor::new(&wire)).expect("decode"),
        error
    );

    for header in [
        "{}",
        r#"{"ok":{},"error":{"class":"failed","code":"x","message":"m","address":null,"retriable":true}}"#,
        r#"{"error":{"class":"failed","code":"x","message":"m","address":null,"retriable":true},"omitted_entries":1}"#,
        r#"{"ok":{},"extra":1}"#,
    ] {
        let mut wire = Vec::new();
        write_message(&mut wire, header.as_bytes(), b"").expect("encode");
        assert!(read_response(&mut Cursor::new(&wire)).is_err(), "{header}");
    }
}

#[test]
fn an_error_code_maps_back_through_the_closed_table_and_an_unknown_one_is_a_protocol_failure() {
    let known = ErrorBody {
        class: ErrorClass::Exhausted,
        code: "workspace.read-limit".to_owned(),
        message: "Requested read exceeds the probed limit.".to_owned(),
        address: Some("limit".to_owned()),
        retriable: true,
    }
    .into_driver_error();
    assert_eq!(known.class, DriverErrorClass::Exhausted);
    assert_eq!(known.code, "workspace.read-limit");
    assert_eq!(known.address.as_deref(), Some("limit"));

    let unknown = ErrorBody {
        class: ErrorClass::Refused,
        code: "workspace.invented-by-a-forger".to_owned(),
        message: "anything".to_owned(),
        address: Some("path".to_owned()),
        retriable: false,
    }
    .into_driver_error();
    assert_eq!(unknown.class, DriverErrorClass::Failed);
    assert_eq!(unknown.code, PROTOCOL_FAILURE);
    assert_eq!(PROTOCOL_FAILURE, "kubernetes.executor-protocol");
}

#[test]
fn the_handshake_is_one_json_line_naming_the_protocol_and_this_release() {
    let handshake = Handshake::current();
    let line = handshake.line();
    assert!(!line.contains('\n'));
    let value: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(value["protocol"], "substrate-executor/1");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert!(value["source"].is_string());
    assert_eq!(value.as_object().expect("object").len(), 3);
    assert!(Handshake::parse(&line).expect("parse").is_current());

    let other_version = line.replace(env!("CARGO_PKG_VERSION"), "0.0.0-other");
    assert!(
        !Handshake::parse(&other_version)
            .expect("parse")
            .is_current()
    );
    let other_protocol = line.replace("substrate-executor/1", "substrate-executor/2");
    assert!(
        !Handshake::parse(&other_protocol)
            .expect("parse")
            .is_current()
    );
    assert!(Handshake::parse(r#"{"protocol":"substrate-executor/1"}"#).is_err());
}
