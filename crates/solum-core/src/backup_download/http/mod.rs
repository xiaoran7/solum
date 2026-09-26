//! `http` -- the production ureq client for the three cloud traits, and the bounded reader.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::stage::{valid_lower_hex, valid_uuid};
use super::*;

/// Production reader with bounded response bodies and the same one-refresh /
/// one-transport-retry account semantics as the uploader.
pub struct HttpBackupCloudReader {
    agent: ureq::Agent,
}

impl Default for HttpBackupCloudReader {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpBackupCloudReader {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build(),
        }
    }

    fn request<F>(&self, session: &mut AccountSession, send: F) -> DownloadResult<ureq::Response>
    where
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        self.request_with_transport_policy(session, true, false, send)
    }

    pub(super) fn request_without_transport_retry<F>(
        &self,
        session: &mut AccountSession,
        send: F,
    ) -> DownloadResult<ureq::Response>
    where
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        self.request_with_transport_policy(session, false, true, send)
    }

    fn request_with_transport_policy<F>(
        &self,
        session: &mut AccountSession,
        retry_transport: bool,
        map_delete_precondition: bool,
        send: F,
    ) -> DownloadResult<ureq::Response>
    where
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        let mut refreshed = false;
        let mut retried_transport = false;
        loop {
            match send(session) {
                Ok(response) => return Ok(response),
                Err(HttpFailure::Status(401)) if !refreshed => {
                    *session = crate::account::refresh_and_save(session)
                        .map_err(|_| BackupDownloadError::CloudAuthentication)?;
                    refreshed = true;
                }
                Err(HttpFailure::Transport) if retry_transport && !retried_transport => {
                    retried_transport = true
                }
                Err(HttpFailure::Transport) => return Err(BackupDownloadError::Transport),
                Err(HttpFailure::Status(401)) => {
                    return Err(BackupDownloadError::CloudAuthentication)
                }
                Err(HttpFailure::Status(404)) => return Err(BackupDownloadError::BackupNotFound),
                Err(HttpFailure::Status(409)) => return Err(BackupDownloadError::CloudConflict),
                Err(HttpFailure::Status(412 | 428)) if map_delete_precondition => {
                    return Err(BackupDownloadError::DeletePrecondition)
                }
                Err(HttpFailure::Status(status)) => {
                    return Err(BackupDownloadError::ServerStatus(status))
                }
            }
        }
    }

    fn url(session: &AccountSession, path: &str) -> String {
        format!("{}{}", session.server_url, path)
    }
}

#[derive(Serialize)]
struct TakeoverRequest<'a> {
    expected_generation: u64,
    takeover_id: &'a str,
    writer_id: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TakeoverResponse {
    active_generation: u64,
    pruned_pending: usize,
    replay: bool,
}

impl BackupCloudTakeover for HttpBackupCloudReader {
    fn advance_generation(
        &self,
        session: &mut AccountSession,
        expected_generation: u64,
        takeover_id: &str,
        writer_id: &str,
    ) -> DownloadResult<CloudTakeoverOutcome> {
        if expected_generation == 0
            || expected_generation >= MAX_SAFE_INTEGER
            || !valid_uuid(takeover_id)
            || !valid_uuid(writer_id)
            || writer_id != writer_id.to_ascii_lowercase()
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let request = TakeoverRequest {
            expected_generation,
            takeover_id,
            writer_id,
        };
        let response = self.request(session, |current| {
            self.agent
                .post(&Self::url(current, "/v1/backups/generation"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .send_json(&request)
                .map_err(HttpFailure::from)
        })?;
        if response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        let bytes = read_bounded(response.into_reader(), 16 * 1024)?;
        let parsed: TakeoverResponse =
            serde_json::from_slice(&bytes).map_err(|_| BackupDownloadError::InvalidResponse)?;
        if parsed.active_generation != expected_generation + 1
            || parsed.pruned_pending > MAX_CATALOG_BACKUPS
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        Ok(CloudTakeoverOutcome {
            active_generation: parsed.active_generation,
            pruned_pending: parsed.pruned_pending,
            replay: parsed.replay,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum HttpFailure {
    Status(u16),
    Transport,
}

impl From<ureq::Error> for HttpFailure {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Status(status, _) => Self::Status(status),
            ureq::Error::Transport(_) => Self::Transport,
        }
    }
}

impl BackupCloudReader for HttpBackupCloudReader {
    fn list_backups(
        &self,
        session: &mut AccountSession,
        limit: usize,
    ) -> DownloadResult<BackupCatalog> {
        if !(1..=MAX_CATALOG_BACKUPS).contains(&limit) {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let path = format!("/v1/backups?limit={limit}");
        let response = self.request(session, |current| {
            self.agent
                .get(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        if response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        let bytes = read_bounded(response.into_reader(), MAX_CATALOG_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|_| BackupDownloadError::InvalidResponse)
    }

    fn download_object(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        object: &BackupObjectPlan,
    ) -> DownloadResult<Vec<u8>> {
        if !valid_uuid(backup_id)
            || !valid_uuid(&object.object_id)
            || object.bytes == 0
            || object.bytes > MAX_CLOUD_OBJECT_BYTES
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let path = format!("/v1/backups/{backup_id}/objects/{}", object.object_id);
        let response = self.request(session, |current| {
            self.agent
                .get(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next());
        let content_length = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        let etag = response.header("ETag");
        let expected_etag = format!("\"{}\"", object.sha256);
        if content_type != Some("application/octet-stream")
            || response.header("Cache-Control") != Some("no-store")
            || content_length != Some(object.bytes)
            || etag != Some(expected_etag.as_str())
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        read_bounded(response.into_reader(), object.bytes as usize)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBackupResponse {
    backup_id: String,
    deleted: bool,
}

impl BackupCloudDelete for HttpBackupCloudReader {
    fn delete_backup(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        plan_etag: &str,
    ) -> DownloadResult<()> {
        if !valid_uuid(backup_id)
            || backup_id != backup_id.to_ascii_lowercase()
            || !valid_lower_hex(plan_etag, 32)
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let path = format!("/v1/backups/{backup_id}");
        let if_match = format!("\"{plan_etag}\"");
        // A transport error can happen after the server committed the hard
        // delete but before the response arrived. Retrying that DELETE would
        // turn a success into a misleading 404, so only a proven 401 refresh
        // is retried. The UI refreshes the catalog after an uncertain result.
        let response = self.request_without_transport_retry(session, |current| {
            self.agent
                .delete(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("If-Match", &if_match)
                .call()
                .map_err(HttpFailure::from)
        })?;
        if response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        let bytes = read_bounded(response.into_reader(), 16 * 1024)?;
        let parsed: DeleteBackupResponse =
            serde_json::from_slice(&bytes).map_err(|_| BackupDownloadError::InvalidResponse)?;
        if parsed.backup_id != backup_id || !parsed.deleted {
            return Err(BackupDownloadError::InvalidResponse);
        }
        Ok(())
    }
}

pub(super) fn read_bounded(mut reader: impl Read, max: usize) -> DownloadResult<Vec<u8>> {
    let limit = u64::try_from(max)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(BackupDownloadError::SizeLimit)?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| BackupDownloadError::Transport)?;
    if bytes.len() > max {
        return Err(BackupDownloadError::SizeLimit);
    }
    Ok(bytes)
}
