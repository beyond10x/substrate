//! Design 34 § 4 and slice U3: the backend ledger's store cases. Names are reserved in the
//! transaction that commits `accepted`, a uid binds once, a second uid is a collision the row
//! survives unchanged, and a settled row is closed.

use chrono::TimeZone as _;
use rusqlite::params;
use substrate_wire::ExecState;
use tempfile::tempdir;

use super::{
    BackendObject, BackendObjectKind, BackendObjectName, BackendObjectRole, BackendReservation,
    BackendUidBinding,
};
use crate::tests::{exec, operation_named, scope, workspace};
use crate::{Reservation, Store, StoreError};

const DEPLOYMENT: &str = "dep_test";
const WORKSPACE: &str = "ws_backend";
const EXEC: &str = "ex_backend";
const ACCEPTED_AT: &str = "2026-08-13T12:00:00Z";

fn names(digest: &str, objects: &[(BackendObjectRole, &str)]) -> BackendReservation {
    BackendReservation {
        operation_digest: digest.to_owned(),
        objects: objects
            .iter()
            .map(|(role, name)| BackendObjectName {
                role: *role,
                name: (*name).to_owned(),
            })
            .collect(),
    }
}

fn accepted_at() -> chrono::DateTime<chrono::Utc> {
    ACCEPTED_AT.parse().expect("time")
}

fn workspace_create(store: &Store, reservation: &BackendReservation) -> Reservation {
    let new = operation_named(
        "local:1000",
        "01JBACKENDWORKSPACE00001",
        "workspace.create",
        WORKSPACE,
        "sha256:workspace",
    );
    store
        .reserve_workspace_create_with_backend(
            &new,
            WORKSPACE,
            &workspace(WORKSPACE),
            None,
            reservation,
        )
        .expect("workspace reservation")
}

fn operation_state(store: &Store, operation: &str) -> Option<String> {
    store
        .connection
        .lock()
        .query_row(
            "SELECT state FROM operations WHERE deployment = ?1 AND operation = ?2",
            params![DEPLOYMENT, operation],
            |row| row.get(0),
        )
        .ok()
}

fn row_count(store: &Store) -> i64 {
    store
        .connection
        .lock()
        .query_row("SELECT COUNT(*) FROM backend_objects", [], |row| row.get(0))
        .expect("row count")
}

#[test]
fn names_are_reserved_in_the_transaction_that_records_accepted() {
    let store = Store::open(":memory:").expect("store");
    let reservation = names(
        &"a".repeat(64),
        &[
            (BackendObjectRole::Pvc, "sbx-ws-h"),
            (BackendObjectRole::Keeper, "sbx-ws-h-keeper"),
        ],
    );
    assert_eq!(
        workspace_create(&store, &reservation),
        Reservation::Accepted
    );
    assert_eq!(
        operation_state(&store, "01JBACKENDWORKSPACE00001").as_deref(),
        Some("accepted")
    );
    let rows = store
        .backend_objects(DEPLOYMENT, BackendObjectKind::Workspace, WORKSPACE)
        .expect("rows");
    let expected = |role, name: &str| BackendObject {
        kind: BackendObjectKind::Workspace,
        resource_id: WORKSPACE.to_owned(),
        role,
        name: name.to_owned(),
        operation_digest: "a".repeat(64),
        uid: None,
        created_at: accepted_at(),
        settled_at: None,
    };
    assert_eq!(
        rows,
        vec![
            expected(BackendObjectRole::Keeper, "sbx-ws-h-keeper"),
            expected(BackendObjectRole::Pvc, "sbx-ws-h"),
        ]
    );

    // A replay of the same operation reserves nothing again.
    assert!(matches!(
        workspace_create(&store, &reservation),
        Reservation::Pending(_)
    ));
    assert_eq!(row_count(&store), 2);

    // An exec start reserves its Pod name with its own acceptance.
    let new = operation_named(
        "local:1000",
        "01JBACKENDEXEC000000001",
        "exec.start",
        EXEC,
        "sha256:exec",
    );
    store
        .complete_workspace(
            &scope("local:1000"),
            "01JBACKENDWORKSPACE00001",
            ACCEPTED_AT,
            201,
            WORKSPACE,
            &workspace(WORKSPACE),
        )
        .expect("workspace ready");
    assert_eq!(
        store
            .reserve_exec_start_with_backend(
                &new,
                &exec(EXEC, WORKSPACE, ExecState::Accepted),
                None,
                None,
                &[],
                &names(&"b".repeat(64), &[(BackendObjectRole::Pod, "sbx-ex-h")]),
            )
            .expect("exec reservation"),
        Reservation::Accepted
    );
    let pod = store
        .backend_objects(DEPLOYMENT, BackendObjectKind::Exec, EXEC)
        .expect("exec rows");
    assert_eq!(pod.len(), 1);
    assert_eq!(pod[0].role, BackendObjectRole::Pod);
    assert_eq!(pod[0].operation_digest, "b".repeat(64));
    assert_eq!(pod[0].uid, None);
}

#[test]
fn a_reservation_without_names_writes_no_row() {
    let store = Store::open(":memory:").expect("store");
    assert_eq!(
        workspace_create(&store, &BackendReservation::default()),
        Reservation::Accepted
    );
    assert_eq!(row_count(&store), 0);
}

#[test]
fn a_held_name_refuses_the_whole_acceptance() {
    let store = Store::open(":memory:").expect("store");
    store
        .reserve_backend_probe(
            DEPLOYMENT,
            "nonce",
            accepted_at(),
            &names("c", &[(BackendObjectRole::Pvc, "sbx-ws-h")]),
        )
        .expect("probe reservation");
    let refused = store
        .reserve_workspace_create_with_backend(
            &operation_named(
                "local:1000",
                "01JBACKENDWORKSPACE00001",
                "workspace.create",
                WORKSPACE,
                "sha256:workspace",
            ),
            WORKSPACE,
            &workspace(WORKSPACE),
            None,
            &names("d", &[(BackendObjectRole::Pvc, "sbx-ws-h")]),
        )
        .expect_err("a held name was reserved twice");
    assert!(
        matches!(&refused, StoreError::BackendNameHeld(name) if name == "sbx-ws-h"),
        "{refused:?}"
    );
    assert_eq!(operation_state(&store, "01JBACKENDWORKSPACE00001"), None);
    assert!(
        store
            .workspace(&scope("local:1000"), WORKSPACE)
            .expect("workspace read")
            .is_none()
    );
    assert_eq!(row_count(&store), 1);
}

#[test]
fn a_uid_binds_once_and_a_second_uid_is_refused() {
    let store = Store::open(":memory:").expect("store");
    workspace_create(&store, &names("e", &[(BackendObjectRole::Pvc, "sbx-ws-h")]));
    assert_eq!(
        store
            .bind_backend_uid(DEPLOYMENT, "sbx-ws-h", "uid-a")
            .expect("bind"),
        BackendUidBinding::Bound
    );
    assert_eq!(
        store
            .bind_backend_uid(DEPLOYMENT, "sbx-ws-h", "uid-a")
            .expect("rebind"),
        BackendUidBinding::Bound
    );
    assert_eq!(
        store
            .bind_backend_uid(DEPLOYMENT, "sbx-ws-h", "uid-b")
            .expect("second uid"),
        BackendUidBinding::Collision {
            bound: "uid-a".to_owned()
        }
    );
    let rows = store
        .backend_objects(DEPLOYMENT, BackendObjectKind::Workspace, WORKSPACE)
        .expect("rows");
    assert_eq!(rows[0].uid.as_deref(), Some("uid-a"));
    assert_eq!(
        store
            .bind_backend_uid(DEPLOYMENT, "sbx-unknown", "uid-a")
            .expect("unknown name"),
        BackendUidBinding::Unrecorded
    );
    assert_eq!(
        store
            .bind_backend_uid("dep_other", "sbx-ws-h", "uid-a")
            .expect("other deployment"),
        BackendUidBinding::Unrecorded
    );
}

#[test]
fn a_settled_row_is_closed_and_binds_nothing() {
    let store = Store::open(":memory:").expect("store");
    workspace_create(
        &store,
        &names(
            "f",
            &[
                (BackendObjectRole::Pvc, "sbx-ws-h"),
                (BackendObjectRole::Keeper, "sbx-ws-h-keeper"),
            ],
        ),
    );
    let first = chrono::Utc
        .with_ymd_and_hms(2026, 8, 13, 12, 5, 0)
        .single()
        .expect("time");
    assert!(
        store
            .settle_backend_object(DEPLOYMENT, "sbx-ws-h-keeper", first)
            .expect("settle")
    );
    assert!(
        store
            .settle_backend_object(
                DEPLOYMENT,
                "sbx-ws-h-keeper",
                first + chrono::Duration::hours(1)
            )
            .expect("settle again")
    );
    assert!(
        !store
            .settle_backend_object(DEPLOYMENT, "sbx-unknown", first)
            .expect("settle unknown")
    );
    assert_eq!(
        store
            .bind_backend_uid(DEPLOYMENT, "sbx-ws-h-keeper", "uid-k")
            .expect("bind settled"),
        BackendUidBinding::Settled
    );
    let keeper = store
        .backend_objects(DEPLOYMENT, BackendObjectKind::Workspace, WORKSPACE)
        .expect("rows")
        .into_iter()
        .find(|row| row.role == BackendObjectRole::Keeper)
        .expect("keeper row");
    assert_eq!(keeper.settled_at, Some(first));
    assert_eq!(keeper.uid, None);
    let unsettled = store
        .unsettled_backend_objects(DEPLOYMENT, None, 16)
        .expect("unsettled");
    assert_eq!(
        unsettled
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sbx-ws-h"]
    );
}

#[test]
fn probe_rows_are_listed_unsettled_oldest_first_within_the_limit_and_deployment() {
    let store = Store::open(":memory:").expect("store");
    let earlier = accepted_at();
    let later = earlier + chrono::Duration::seconds(1);
    store
        .reserve_backend_probe(
            DEPLOYMENT,
            "nonce-b",
            later,
            &names(
                "g",
                &[
                    (BackendObjectRole::Pod, "sbx-pr-b-pod"),
                    (BackendObjectRole::Pvc, "sbx-pr-b-pvc"),
                ],
            ),
        )
        .expect("probe b");
    store
        .reserve_backend_probe(
            DEPLOYMENT,
            "nonce-a",
            earlier,
            &names("h", &[(BackendObjectRole::Keeper, "sbx-pr-a-keeper")]),
        )
        .expect("probe a");
    store
        .reserve_backend_probe(
            "dep_other",
            "nonce-c",
            earlier,
            &names("i", &[(BackendObjectRole::Pod, "sbx-pr-c-pod")]),
        )
        .expect("probe c");
    let listed = store
        .unsettled_backend_objects(DEPLOYMENT, None, 2)
        .expect("unsettled");
    assert_eq!(
        listed
            .iter()
            .map(|row| (row.kind, row.resource_id.as_str(), row.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (BackendObjectKind::Probe, "nonce-a", "sbx-pr-a-keeper"),
            (BackendObjectKind::Probe, "nonce-b", "sbx-pr-b-pod"),
        ]
    );
}

/// The table is a forward-only migration: a store stamped by the previous schema, without the
/// table, opens, gains it and is stamped current; rows survive a restart.
#[test]
fn an_earlier_store_gains_the_table_and_rows_survive_a_restart() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    drop(Store::open(&path).expect("first open"));
    {
        let earlier = rusqlite::Connection::open(&path).expect("connection");
        earlier
            .execute_batch("DROP TABLE backend_objects; PRAGMA user_version = 1;")
            .expect("rewind to the previous schema");
    }
    let store = Store::open(&path).expect("upgrade open");
    assert_eq!(
        store
            .schema_version(std::time::Duration::from_secs(1))
            .expect("version"),
        Some(crate::SCHEMA_VERSION)
    );
    const { assert!(crate::SCHEMA_VERSION >= 2) };
    workspace_create(&store, &names("j", &[(BackendObjectRole::Pvc, "sbx-ws-h")]));
    store
        .bind_backend_uid(DEPLOYMENT, "sbx-ws-h", "uid-a")
        .expect("bind");
    drop(store);
    let reopened = Store::open(&path).expect("restart");
    let rows = reopened
        .backend_objects(DEPLOYMENT, BackendObjectKind::Workspace, WORKSPACE)
        .expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uid.as_deref(), Some("uid-a"));
}

/// More unsettled rows than one page: the cursor reaches every row once, in order, including
/// rows that share a `created_at`, and a settled row is skipped.
#[test]
fn the_unsettled_cursor_reaches_rows_past_the_first_page() {
    const PAGE: usize = 256;
    const ROWS: usize = 300;
    let store = Store::open(":memory:").expect("store");
    let base = accepted_at();
    for index in 0..ROWS {
        let created_at = base + chrono::Duration::seconds(i64::try_from(index / 2).expect("i64"));
        store
            .reserve_backend_probe(
                DEPLOYMENT,
                &format!("nonce-{index:03}"),
                created_at,
                &names(
                    "k",
                    &[(BackendObjectRole::Pod, &format!("sbx-pr-{index:03}-pod"))],
                ),
            )
            .expect("probe row");
    }
    assert!(
        store
            .settle_backend_object(DEPLOYMENT, "sbx-pr-005-pod", base)
            .expect("settle")
    );
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .unsettled_backend_objects(DEPLOYMENT, after.as_deref(), PAGE)
            .expect("page");
        assert!(page.len() <= PAGE);
        let last = page.len() < PAGE;
        after = page.last().map(|row| row.name.clone());
        seen.extend(page.into_iter().map(|row| row.name));
        if last {
            break;
        }
        assert!(seen.len() <= ROWS, "the cursor did not advance");
    }
    let expected: Vec<String> = (0..ROWS)
        .filter(|index| *index != 5)
        .map(|index| format!("sbx-pr-{index:03}-pod"))
        .collect();
    assert_eq!(seen, expected);
}
