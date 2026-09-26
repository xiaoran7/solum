//! http. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! The production HTTP adapter for the transport seam, with its private wire structs.

use super::staging::{sha256_hex, valid_lower_hex, valid_server_timestamp, valid_uuid};
use super::*;
use serde::{Deserialize, Serialize};

/// Production HTTP adapter. A 401 rotates and durably saves the account once;
/// one transport failure is retried. Status responses are never read into an
/// error message, so the server cannot reflect sensitive content into logs.
pub struct HttpBackupCloudTransport {
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Copy)]
enum HttpFailure {
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

impl Default for HttpBackupCloudTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpBackupCloudTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build(),
        }
    }

    fn request_json<T, F>(&self, session: &mut AccountSession, send: F) -> UploadResult<T>
    where
        T: DeserializeOwned,
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        let response = self.request(session, send)?;
        response
            .into_json::<T>()
            .map_err(|_| BackupUploadError::InvalidResponse)
    }

    fn request<F>(&self, session: &mut AccountSession, send: F) -> UploadResult<ureq::Response>
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
                        .map_err(|_| BackupUploadError::Authentication)?;
                    refreshed = true;
                }
                Err(HttpFailure::Transport) if !retried_transport => {
                    retried_transport = true;
                }
                Err(HttpFailure::Transport) => return Err(BackupUploadError::Transport),
                Err(HttpFailure::Status(401)) => return Err(BackupUploadError::Authentication),
                Err(HttpFailure::Status(status)) => {
                    return Err(match status {
                        409 => BackupUploadError::CloudConflict,
                        413 => BackupUploadError::SizeLimit,
                        _ => BackupUploadError::ServerStatus(status),
                    })
                }
            }
        }
    }

    fn url(session: &AccountSession, path: &str) -> String {
        format!("{}{}", session.server_url, path)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationResponse {
    active_generation: u64,
    updated_at: String,
}

#[derive(Serialize)]
struct WriterClaimRequest<'a> {
    expected_generation: u64,
    writer_id: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriterClaimResponse {
    active_generation: u64,
    claimed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResponse {
    backup_id: String,
    generation: u64,
    status: String,
    plan_etag: String,
    total_bytes: u64,
    object_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutResponse {
    backup_id: String,
    object_id: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitResponse {
    backup_id: String,
    generation: u64,
    created_at: String,
    committed_at: String,
    protocol_version: u32,
    total_bytes: u64,
    object_count: usize,
    plan_etag: String,
    objects: Vec<BackupObjectPlan>,
    status: String,
    pruned_versions: Vec<String>,
}

impl BackupCloudTransport for HttpBackupCloudTransport {
    fn active_generation(&self, session: &mut AccountSession) -> UploadResult<u64> {
        let response: GenerationResponse = self.request_json(session, |current| {
            self.agent
                .get(&Self::url(current, "/v1/backups/generation"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        if response.active_generation == 0 || !valid_server_timestamp(&response.updated_at) {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(response.active_generation)
    }

    fn claim_initial_writer(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
    ) -> UploadResult<u64> {
        if !valid_uuid(writer_id) || writer_id != writer_id.to_ascii_lowercase() {
            return Err(BackupUploadError::InvalidDeviceState);
        }
        let request = WriterClaimRequest {
            expected_generation: 1,
            writer_id,
        };
        let response: WriterClaimResponse = self.request_json(session, |current| {
            self.agent
                .post(&Self::url(current, "/v1/backups/writer"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .send_json(&request)
                .map_err(HttpFailure::from)
        })?;
        if response.active_generation != 1 {
            return Err(BackupUploadError::InvalidResponse);
        }
        let _ = response.claimed;
        Ok(response.active_generation)
    }

    fn create_plan(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        plan: &BackupPlan,
    ) -> UploadResult<()> {
        let response: CreateResponse = self.request_json(session, |current| {
            self.agent
                .post(&Self::url(current, "/v1/backups"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("X-Solum-Backup-Writer", writer_id)
                .send_json(plan)
                .map_err(HttpFailure::from)
        })?;
        let total_bytes = plan.objects.iter().try_fold(0u64, |sum, object| {
            sum.checked_add(object.bytes)
                .ok_or(BackupUploadError::SizeLimit)
        })?;
        if response.backup_id != plan.backup_id
            || response.generation != plan.generation
            || !matches!(response.status.as_str(), "pending" | "complete")
            || !valid_lower_hex(&response.plan_etag, 32)
            || response.total_bytes != total_bytes
            || response.object_count != plan.objects.len()
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(())
    }

    fn put_object(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        backup_id: &str,
        object: &BackupObjectPlan,
        ciphertext: &[u8],
    ) -> UploadResult<()> {
        if ciphertext.len() as u64 != object.bytes || sha256_hex(ciphertext) != object.sha256 {
            return Err(BackupUploadError::Staging);
        }
        let path = format!("/v1/backups/{backup_id}/objects/{}", object.object_id);
        let response: PutResponse = self.request_json(session, |current| {
            self.agent
                .put(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("X-Solum-Backup-Writer", writer_id)
                .set("Content-Type", "application/octet-stream")
                .set("Content-Length", &ciphertext.len().to_string())
                .send_bytes(ciphertext)
                .map_err(HttpFailure::from)
        })?;
        if response.backup_id != backup_id
            || response.object_id != object.object_id
            || response.sha256 != object.sha256
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(())
    }

    fn commit(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        plan: &BackupPlan,
    ) -> UploadResult<BackupUploadOutcome> {
        let path = format!("/v1/backups/{}/commit", plan.backup_id);
        let response: CommitResponse = self.request_json(session, |current| {
            self.agent
                .post(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("X-Solum-Backup-Writer", writer_id)
                .set("Content-Length", "0")
                .send_bytes(&[])
                .map_err(HttpFailure::from)
        })?;
        let total_bytes = plan.objects.iter().map(|object| object.bytes).sum::<u64>();
        if response.backup_id != plan.backup_id
            || response.generation != plan.generation
            || response.created_at != plan.created_at
            || !valid_server_timestamp(&response.committed_at)
            || response.protocol_version != plan.protocol_version
            || response.total_bytes != total_bytes
            || response.object_count != plan.objects.len()
            || !valid_lower_hex(&response.plan_etag, 32)
            || !response.objects.is_empty()
            || response.status != "complete"
            || response
                .pruned_versions
                .iter()
                .any(|value| !valid_uuid(value) || value != &value.to_ascii_lowercase())
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(BackupUploadOutcome {
            backup_id: response.backup_id,
            generation: response.generation,
            object_count: response.object_count,
        })
    }
}
