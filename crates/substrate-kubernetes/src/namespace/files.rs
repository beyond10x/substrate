//! The file routes (design 34 § 5 second row, § 7.2): each is one `substrate-executor fs` call in
//! the workspace's keeper through `pods/exec`, which runs the host driver's own guarded `openat2`
//! code against the PVC. No shell performs file I/O.
//!
//! **A lost answer is resolved once, never resent (D10).** When a mutation's call goes unanswered,
//! one `stat` call follows. For an unconditional write or delete, if the file's digest is the one
//! the request would have left — the written content's, or absence — that observation is the
//! answer; otherwise the answer is `Failed` `kubernetes.executor-unanswered`, retriable. A request
//! with a precondition (a replace's create-if-absent or expected digest, an edit's or a patch's
//! expected digest) always ends so: the state it would leave is also the state another writer can
//! leave while this call is refused, so no `stat` tells the two apart. Its retry is safe, because
//! an applied call's precondition no longer holds. A read that goes unanswered is that failure at
//! once: there is nothing to resolve.

use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use substrate_executor::message::MAX_HEADER_BYTES;
use substrate_executor::message::{PROTOCOL_FAILURE, Request, Response, read_response};
use substrate_host::{DriverError, Listed};
use substrate_kubernetes_client::ApiError;
use substrate_wire::{
    DigestedFileSlice, FileAbsence, FileEditInput, FileKind, FileMutationResult, FileObservation,
    FilePatchInput, FileReadQuery, FileReadResult, FileReplaceInput, WorkspaceTree,
    WorkspaceTreeQuery,
};

use super::NamespaceDriver;
use super::workspace::{EXECUTOR_PATH, KEEPER_CONTAINER};
use crate::API_FAILED;
use crate::ownership::{Ownership, ownership};
use crate::policy::hex;

/// A file call whose answer was lost and whose one `stat` did not show the requested state
/// (design 34 § 8): retriable, and nothing was sent again.
pub const EXECUTOR_UNANSWERED: &str = "kubernetes.executor-unanswered";

/// The most bytes one `fs` answer may hold: the framing's two lengths and a header at the
/// protocol's bound (§ 7.2). Content is never answered.
const RESPONSE_BYTES: usize = 4 + MAX_HEADER_BYTES as usize + 8;

/// Why one `fs` call has no answer to decode.
enum CallFailure {
    /// No answer came: the call may have run.
    Unanswered(String),
    /// Anything else, already the route's answer.
    Failed(DriverError),
}

/// What a mutation leaves when it is applied, for the one `stat` after a lost answer.
enum Expected {
    /// A regular file with this complete-content digest and size.
    Digest { sha256: String, size: u64 },
    /// No file.
    Absent,
    /// No `stat` may become the answer: the request carries a precondition, so the state it
    /// would leave cannot tell an applied call from a refused one, or it is not a mutation.
    Unresolvable,
}

impl Expected {
    /// The state `request` leaves when applied, if a `stat` may answer for it. Only an
    /// unconditional write or delete qualifies; every request with a precondition (create-if-absent
    /// or an expected digest) does not, whatever it would leave. The match names every request so
    /// a new one is classified here before it compiles.
    fn of(request: &Request, content: &[u8]) -> Self {
        match request {
            Request::Write { .. } => Self::Digest {
                sha256: hex(&Sha256::digest(content)),
                size: u64::try_from(content.len()).unwrap_or(u64::MAX),
            },
            Request::Delete { .. } => Self::Absent,
            Request::ReplaceV2 { .. }
            | Request::EditV2 { .. }
            | Request::PatchV2 { .. }
            | Request::Read { .. }
            | Request::ReadV2 { .. }
            | Request::TreeV2 { .. }
            | Request::Stat { .. }
            | Request::Check {} => Self::Unresolvable,
        }
    }
}

/// What a `stat` call answered.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Stat {
    Present { size: u64, sha256: String },
    Absent { absent: bool },
}

impl NamespaceDriver {
    /// `workspace.file.read`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, or the call's failure.
    pub async fn file_read(
        &self,
        workspace: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<Listed<FileReadResult>, DriverError> {
        let request = Request::Read {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            query: query.clone(),
        };
        self.read_call(workspace, &request).await
    }

    /// `workspace.file.read-v2`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, or the call's failure.
    pub async fn file_read_v2(
        &self,
        workspace: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<DigestedFileSlice, DriverError> {
        let request = Request::ReadV2 {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            query: query.clone(),
        };
        self.read_call(workspace, &request)
            .await
            .map(|listed| listed.value)
    }

    /// `workspace.tree.read-v2`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, or the call's failure.
    pub async fn tree_read_v2(
        &self,
        workspace: &str,
        query: &WorkspaceTreeQuery,
    ) -> Result<Listed<WorkspaceTree>, DriverError> {
        let request = Request::TreeV2 {
            workspace: workspace.to_owned(),
            query: query.clone(),
        };
        self.read_call(workspace, &request).await
    }

    /// `workspace.file.write`: the complete new file.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, the call's failure, or `kubernetes.executor-unanswered`.
    pub async fn file_write(
        &self,
        workspace: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        let request = Request::Write {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
        };
        match self
            .mutation_call(workspace, path, &request, content)
            .await?
        {
            Resolution::Answered(value) => decode(value),
            Resolution::Stated { sha256, size } => Ok(FileObservation {
                kind: FileKind::File,
                workspace: workspace.to_owned(),
                path: path.to_owned(),
                size,
                sha256,
                atomic_replacement: true,
                observed_at: Utc::now(),
            }),
            Resolution::Absent => Err(unanswered()),
        }
    }

    /// `workspace.file.delete`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, the call's failure, or `kubernetes.executor-unanswered`.
    pub async fn file_delete(
        &self,
        workspace: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        let request = Request::Delete {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
        };
        match self.mutation_call(workspace, path, &request, &[]).await? {
            Resolution::Answered(value) => decode(value),
            Resolution::Absent => Ok(FileAbsence {
                kind: FileKind::File,
                workspace: workspace.to_owned(),
                path: path.to_owned(),
                absent: true,
                observed_at: Utc::now(),
            }),
            Resolution::Stated { .. } => Err(unanswered()),
        }
    }

    /// `workspace.file.replace-v2`. Every replacement is conditional (create-if-absent or an
    /// expected digest), so a lost answer is `kubernetes.executor-unanswered` whatever its `stat`
    /// shows: the replacing content on disk does not say whether this call applied it or another
    /// writer did and this call was refused. A retry is safe: after an applied call its
    /// precondition no longer holds.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, the call's failure, or `kubernetes.executor-unanswered`.
    pub async fn file_replace_v2(
        &self,
        workspace: &str,
        path: &str,
        input: &FileReplaceInput,
    ) -> Result<FileMutationResult, DriverError> {
        let request = Request::ReplaceV2 {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            input: input.clone(),
        };
        self.conditional_mutation(workspace, path, &request).await
    }

    /// `workspace.file.edit-v2`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, the call's failure, or `kubernetes.executor-unanswered`.
    pub async fn file_edit_v2(
        &self,
        workspace: &str,
        path: &str,
        input: &FileEditInput,
    ) -> Result<FileMutationResult, DriverError> {
        let request = Request::EditV2 {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            input: input.clone(),
        };
        self.conditional_mutation(workspace, path, &request).await
    }

    /// `workspace.file.patch-v2`.
    ///
    /// # Errors
    ///
    /// The guarded code's own refusal, the call's failure, or `kubernetes.executor-unanswered`.
    pub async fn file_patch_v2(
        &self,
        workspace: &str,
        path: &str,
        input: &FilePatchInput,
    ) -> Result<FileMutationResult, DriverError> {
        let request = Request::PatchV2 {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            input: input.clone(),
        };
        self.conditional_mutation(workspace, path, &request).await
    }

    /// A mutation with a precondition: its answer, or `kubernetes.executor-unanswered` when the
    /// answer is lost, whatever the one `stat` shows.
    async fn conditional_mutation(
        &self,
        workspace: &str,
        path: &str,
        request: &Request,
    ) -> Result<FileMutationResult, DriverError> {
        match self.mutation_call(workspace, path, request, &[]).await? {
            Resolution::Answered(value) => decode(value),
            Resolution::Stated { .. } | Resolution::Absent => Err(unanswered()),
        }
    }

    /// One read call: its answer, or the failure an unanswered read is.
    async fn read_call<T: DeserializeOwned>(
        &self,
        workspace: &str,
        request: &Request,
    ) -> Result<Listed<T>, DriverError> {
        match self.fs_call(workspace, request, &[]).await {
            Ok((value, omitted_entries)) => Ok(Listed {
                value: decode(value)?,
                omitted_entries,
            }),
            Err(CallFailure::Unanswered(_)) => Err(unanswered()),
            Err(CallFailure::Failed(error)) => Err(error),
        }
    }

    /// One mutation call; when its answer is lost, one `stat` call and its comparison with what
    /// the request leaves ([`Expected::of`]). A request with a precondition is never resolved by
    /// the `stat`.
    async fn mutation_call(
        &self,
        workspace: &str,
        path: &str,
        request: &Request,
        content: &[u8],
    ) -> Result<Resolution, DriverError> {
        let expected = Expected::of(request, content);
        match self.fs_call(workspace, request, content).await {
            Ok((value, _)) => return Ok(Resolution::Answered(value)),
            Err(CallFailure::Failed(error)) => return Err(error),
            Err(CallFailure::Unanswered(detail)) => {
                tracing::warn!(
                    detail,
                    "a file call went unanswered; one stat call resolves it"
                );
            }
        }
        let stat = Request::Stat {
            workspace: workspace.to_owned(),
            path: path.to_owned(),
        };
        let observed = match self.fs_call(workspace, &stat, &[]).await {
            Ok((value, _)) => decode::<Stat>(value).map_err(|_| unanswered())?,
            Err(_) => return Err(unanswered()),
        };
        Ok(match (expected, observed) {
            (
                Expected::Digest { sha256, size },
                Stat::Present {
                    sha256: seen,
                    size: seen_size,
                },
            ) if sha256 == seen && size == seen_size => Resolution::Stated {
                sha256: seen,
                size: seen_size,
            },
            (Expected::Absent, Stat::Absent { absent: true }) => Resolution::Absent,
            _ => return Err(unanswered()),
        })
    }

    /// One `substrate-executor fs` call in the workspace's keeper: the request message on stdin,
    /// one response message on stdout, and channel 3's `Success`. The keeper must be the one the
    /// workspace's row has bound: a keeper that is gone is never recreated (§ 15), and an object
    /// under its name that is not ours is never called.
    async fn fs_call(
        &self,
        workspace: &str,
        request: &Request,
        content: &[u8],
    ) -> Result<(Value, u64), CallFailure> {
        let keeper = self
            .serving_keeper(workspace)
            .await
            .map_err(CallFailure::Failed)?;
        let mut stdin = Vec::new();
        request.write_to(&mut stdin, content).map_err(|error| {
            CallFailure::Failed(DriverError::failed(
                PROTOCOL_FAILURE,
                format!("the file request does not fit the executor protocol: {error}"),
            ))
        })?;
        let output = match self
            .api
            .exec(
                &keeper,
                KEEPER_CONTAINER,
                &[EXECUTOR_PATH, "fs"],
                &stdin,
                RESPONSE_BYTES,
            )
            .await
        {
            Ok(output) => output,
            Err(ApiError::Unanswered(detail)) => return Err(CallFailure::Unanswered(detail)),
            Err(ApiError::Answered { status: 404, .. }) => {
                return Err(CallFailure::Failed(keeper_gone()));
            }
            Err(error) => {
                return Err(CallFailure::Failed(DriverError::failed(
                    API_FAILED,
                    format!("the keeper's file call: {error}"),
                )));
            }
        };
        let protocol = |why: &str| {
            CallFailure::Failed(DriverError::failed(
                PROTOCOL_FAILURE,
                format!("the keeper's executor answered outside its protocol: {why}"),
            ))
        };
        if output.exit_code() != Some(0) {
            return Err(protocol("its exec status is not Success"));
        }
        if output.stdout_truncated {
            return Err(protocol("its answer is over the response bound"));
        }
        match read_response(&mut output.stdout.as_slice()) {
            Ok(Response::Ok {
                value,
                omitted_entries,
            }) => Ok((value, omitted_entries)),
            Ok(Response::Error(body)) => Err(CallFailure::Failed(body.into_driver_error())),
            Err(error) => Err(protocol(&error.to_string())),
        }
    }

    /// The keeper Pod the workspace's row has bound, read now and still ours.
    async fn serving_keeper(&self, workspace: &str) -> Result<String, DriverError> {
        let (_, keeper) = self.workspace_rows(workspace)?;
        if keeper.uid.is_none() || keeper.settled_at.is_some() {
            return Err(DriverError::failed(
                API_FAILED,
                "the workspace has no keeper bound: it is not ready",
            ));
        }
        let path = format!("{}/{}", self.api.pods(), keeper.name);
        let Some(object) = self
            .api
            .get(&path)
            .await
            .map_err(|error| super::api_failed(&error))?
        else {
            return Err(keeper_gone());
        };
        match ownership(&object, Some(&keeper), &self.deployment_label) {
            Ownership::Ours { adopt: false, .. } => Ok(keeper.name),
            Ownership::Ours { adopt: true, .. } | Ownership::Collision(_) => {
                self.collision(&format!("{:?} is not the bound keeper", keeper.name));
                Err(DriverError::failed(
                    API_FAILED,
                    "the object under the keeper's name is not the bound keeper; it is not called",
                ))
            }
        }
    }
}

/// How a mutation call ended.
enum Resolution {
    /// The executor answered this observation.
    Answered(Value),
    /// The answer was lost and one `stat` showed the requested content.
    Stated { sha256: String, size: u64 },
    /// The answer was lost and one `stat` showed the file absent, as a delete leaves it.
    Absent,
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, DriverError> {
    serde_json::from_value(value).map_err(|_| {
        DriverError::failed(
            PROTOCOL_FAILURE,
            "the keeper's executor answered an observation of another shape",
        )
    })
}

fn unanswered() -> DriverError {
    DriverError::failed(
        EXECUTOR_UNANSWERED,
        "The executor's answer to the file call was lost and one stat did not show the requested \
         state; nothing was sent again.",
    )
}

fn keeper_gone() -> DriverError {
    DriverError::failed(
        API_FAILED,
        "the workspace's keeper is gone; the namespace driver does not recreate it",
    )
}
