//! Design 34 § 7.3: the container-log frame lines the exec Pod's PID 1 writes.

use substrate_executor::frame::{
    Applied, Control, ExitRecord, Frame, MAX_CHUNK_BYTES, MAX_LINE_BYTES, Refusal, Stream,
};

#[test]
fn every_frame_kind_round_trips_through_one_line() {
    let frames = [
        Frame::Output {
            stream: Stream::Stdout,
            bytes: b"plain".to_vec(),
        },
        Frame::Output {
            stream: Stream::Stderr,
            bytes: vec![0, 10, 13, 255, b'$'],
        },
        Frame::Control(Control::Applied(Applied {
            memory_bytes: Some(67_108_864),
            cpu_millicores: Some(500),
            pids_max: Some("max".to_owned()),
        })),
        Frame::Control(Control::Applied(Applied {
            memory_bytes: None,
            cpu_millicores: None,
            pids_max: None,
        })),
        Frame::Control(Control::Refused(Refusal::LimitsUnapplied)),
        Frame::Control(Control::Truncated(Stream::Stdout)),
        Frame::Control(Control::Truncated(Stream::Stderr)),
        Frame::Control(Control::Exit(ExitRecord::Code(3))),
        Frame::Control(Control::Exit(ExitRecord::Signal("term".to_owned()))),
        Frame::Control(Control::SpawnFailed { errno: 2 }),
    ];
    for frame in frames {
        let line = frame.encode().expect("encode");
        assert!(line.starts_with("b10x1 "), "{line}");
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
        assert_eq!(Frame::decode(&line).expect("decode"), frame, "{line}");
    }
}

#[test]
fn the_control_records_have_the_designed_spelling() {
    let spelled = [
        (
            Frame::Control(Control::Refused(Refusal::LimitsUnapplied)),
            r#"b10x1 c {"refused":"limits-unapplied"}"#,
        ),
        (
            Frame::Control(Control::Truncated(Stream::Stdout)),
            r#"b10x1 c {"truncated":"o"}"#,
        ),
        (
            Frame::Control(Control::Exit(ExitRecord::Code(0))),
            r#"b10x1 c {"exit":{"code":0}}"#,
        ),
        (
            Frame::Control(Control::Exit(ExitRecord::Signal("kill".to_owned()))),
            r#"b10x1 c {"exit":{"signal":"kill"}}"#,
        ),
        (
            Frame::Output {
                stream: Stream::Stderr,
                bytes: b"hi".to_vec(),
            },
            "b10x1 e aGk=",
        ),
    ];
    for (frame, line) in spelled {
        assert_eq!(frame.encode().expect("encode"), line);
    }
}

#[test]
fn a_full_chunk_stays_under_the_cri_line_limit_and_a_larger_one_is_refused() {
    let full = Frame::Output {
        stream: Stream::Stdout,
        bytes: vec![0xff; MAX_CHUNK_BYTES],
    };
    let line = full.encode().expect("a full chunk encodes");
    assert!(line.len() + 1 < MAX_LINE_BYTES);
    const { assert!(MAX_LINE_BYTES <= 16 * 1024) };
    assert_eq!(Frame::decode(&line).expect("decode"), full);

    let over = Frame::Output {
        stream: Stream::Stdout,
        bytes: vec![0; MAX_CHUNK_BYTES + 1],
    };
    assert!(over.encode().is_err());
}

#[test]
fn a_line_that_is_not_a_frame_is_refused_never_read_as_output() {
    let too_long = format!("b10x1 o {}", "A".repeat(MAX_LINE_BYTES));
    let over_chunk = format!("b10x1 o {}", base64_of(&vec![1_u8; MAX_CHUNK_BYTES + 3]));
    for line in [
        "",
        "hello from the child",
        "b10x1",
        "b10x1 o",
        "b10x1 o ",
        "b10x1 x aGk=",
        "b10x2 o aGk=",
        "b10x1 o aGk",
        "b10x1 o aGk=\n",
        "b10x1 o  aGk=",
        "b10x1 o a-k_",
        r#"b10x1 c {"exit":{"code":0},"extra":1}"#,
        r#"b10x1 c {"refused":"something-else"}"#,
        r#"b10x1 c {"truncated":"x"}"#,
        "b10x1 c not json",
        too_long.as_str(),
        over_chunk.as_str(),
    ] {
        assert!(Frame::decode(line).is_err(), "{line:?}");
    }
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
