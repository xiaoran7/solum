//! session. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! The coordinator: device-generation authority on disk, and the staged session that plans, uploads and commits.

use super::staging::{
    clean_encrypted_staging, hex, map_attachment_error, portable_manifests, random_hex,
    random_uuid_v4, sha256_hex, stage_ciphertext, valid_uuid,
};
use super::types::{BackupDevicePhase, BackupDeviceState, StagedObject};
use super::*;

/// Load local single-writer authority and compare it to the server. A missing
/// file first persists a resumable random writer claim; the server atomically
/// accepts at most one writer for an empty generation-1 tenant.
pub fn load_or_initialize_device_generation(
    profile_root: &Path,
    user_id: &str,
    transport: &dyn BackupCloudTransport,
    session: &mut AccountSession,
) -> UploadResult<BackupDeviceAuthority> {
    if !valid_uuid(user_id) || user_id != user_id.to_ascii_lowercase() {
        return Err(BackupUploadError::InvalidDeviceState);
    }
    let path = profile_root.join(DEVICE_STATE_FILE);
    let local = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<BackupDeviceState>(&raw)
            .map(Some)
            .map_err(|_| BackupUploadError::InvalidDeviceState)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(BackupUploadError::InvalidDeviceState),
    };
    if let Some(state) = local.as_ref() {
        if state.format != DEVICE_STATE_FORMAT
            || state.version != DEVICE_STATE_VERSION
            || state.user_id != user_id
            || state.device_generation == 0
            || !valid_uuid(&state.writer_id)
            || state.writer_id != state.writer_id.to_ascii_lowercase()
            || (state.phase == BackupDevicePhase::Claiming && state.device_generation != 1)
        {
            return Err(BackupUploadError::InvalidDeviceState);
        }
    }

    let mut state = match local {
        Some(state) => state,
        None => {
            let state = BackupDeviceState {
                format: DEVICE_STATE_FORMAT.to_string(),
                version: DEVICE_STATE_VERSION,
                user_id: user_id.to_string(),
                device_generation: 1,
                writer_id: random_uuid_v4()?,
                phase: BackupDevicePhase::Claiming,
            };
            persist_device_state(&path, &state)?;
            state
        }
    };

    if state.phase == BackupDevicePhase::Claiming {
        let remote = transport.claim_initial_writer(session, &state.writer_id)?;
        if remote != 1 {
            return Err(BackupUploadError::InvalidResponse);
        }
        state.phase = BackupDevicePhase::Active;
        persist_device_state(&path, &state)?;
    } else {
        let remote = transport.active_generation(session)?;
        if remote == 0 {
            return Err(BackupUploadError::InvalidResponse);
        }
        if state.device_generation != remote {
            return Err(BackupUploadError::StaleGeneration);
        }
    }
    Ok(BackupDeviceAuthority {
        generation: state.device_generation,
        writer_id: state.writer_id,
    })
}

fn persist_device_state(path: &Path, state: &BackupDeviceState) -> UploadResult<()> {
    let encoded =
        serde_json::to_string(state).map_err(|_| BackupUploadError::InvalidDeviceState)?;
    crate::fsatomic::write_atomic_durable(path, &encoded)
        .map_err(|_| BackupUploadError::InvalidDeviceState)
}

/// Persist the generation returned by an authenticated, journaled takeover.
/// A new phone may have no local state; an existing authority may still carry
/// the expected generation. Any unrelated value fails closed.
pub fn persist_taken_over_device_generation(
    profile_root: &Path,
    user_id: &str,
    expected_generation: u64,
    target_generation: u64,
    writer_id: &str,
) -> UploadResult<()> {
    if !valid_uuid(user_id)
        || user_id != user_id.to_ascii_lowercase()
        || expected_generation == 0
        || expected_generation.checked_add(1) != Some(target_generation)
        || !valid_uuid(writer_id)
        || writer_id != writer_id.to_ascii_lowercase()
    {
        return Err(BackupUploadError::InvalidDeviceState);
    }
    let path = profile_root.join(DEVICE_STATE_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(raw) => Some(
            serde_json::from_str::<BackupDeviceState>(&raw)
                .map_err(|_| BackupUploadError::InvalidDeviceState)?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(BackupUploadError::InvalidDeviceState),
    };
    if let Some(state) = existing.as_ref() {
        if state.format != DEVICE_STATE_FORMAT
            || state.version != DEVICE_STATE_VERSION
            || state.user_id != user_id
            || !matches!(
                state.device_generation,
                value if value == expected_generation || value == target_generation
            )
            || (state.device_generation == target_generation
                && (state.writer_id != writer_id || state.phase != BackupDevicePhase::Active))
        {
            return Err(BackupUploadError::InvalidDeviceState);
        }
        if state.device_generation == target_generation {
            return Ok(());
        }
    }
    let state = BackupDeviceState {
        format: DEVICE_STATE_FORMAT.to_string(),
        version: DEVICE_STATE_VERSION,
        user_id: user_id.to_string(),
        device_generation: target_generation,
        writer_id: writer_id.to_string(),
        phase: BackupDevicePhase::Active,
    };
    persist_device_state(&path, &state)
}

impl BackupUploadSession {
    pub fn begin(
        profile_root: &Path,
        portable_json: &[u8],
        user_id: &str,
        device_generation: u64,
        writer_id: String,
        created_at: &str,
        master_key: [u8; 32],
    ) -> UploadResult<Self> {
        if !valid_uuid(&writer_id) || writer_id != writer_id.to_ascii_lowercase() {
            return Err(BackupUploadError::InvalidDeviceState);
        }
        let document: Value = serde_json::from_slice(portable_json)
            .map_err(|_| BackupUploadError::InvalidSnapshot)?;
        crate::export::plan_import(&document).map_err(|_| BackupUploadError::InvalidSnapshot)?;
        let manifests = portable_manifests(&document)?;

        let backup_id = random_uuid_v4()?;
        let metadata = BackupMetadata {
            user_id: user_id.to_string(),
            device_generation,
            backup_id,
            created_at: created_at.to_string(),
        };
        let bindings: Vec<_> = manifests
            .iter()
            .map(|manifest| AttachmentBinding::from_manifest(&metadata, manifest))
            .collect();
        attachment::validate_attachment_set(&bindings).map_err(map_attachment_error)?;

        let staging_root = profile_root.join(UPLOAD_STAGING_DIR);
        clean_encrypted_staging(&staging_root)?;
        std::fs::create_dir_all(&staging_root).map_err(|_| BackupUploadError::Staging)?;
        let session_id = random_hex(16)?;
        let staging_dir = staging_root.join(&session_id);
        std::fs::create_dir(&staging_dir).map_err(|_| BackupUploadError::Staging)?;

        let result = (|| {
            let main = backup::create_backup(&document, &metadata, &master_key)
                .map_err(|_| BackupUploadError::Encryption)?;
            let object_id = random_uuid_v4()?;
            let staged_main =
                stage_ciphertext(&staging_dir, object_id, BackupObjectKind::Main, &main)?;
            Ok(Self {
                session_id,
                metadata,
                writer_id,
                master_key,
                manifests,
                next_attachment: 0,
                attachment_plaintext: Vec::new(),
                staged: vec![staged_main],
                staging_dir: staging_dir.clone(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging_dir);
        }
        result
    }

    pub fn begin_info(&self) -> BackupUploadBegin {
        BackupUploadBegin {
            session_id: self.session_id.clone(),
            backup_id: self.metadata.backup_id.clone(),
            generation: self.metadata.device_generation,
            created_at: self.metadata.created_at.clone(),
            attachments: self
                .manifests
                .iter()
                .map(|manifest| AttachmentExpectation {
                    id: manifest.id.clone(),
                    plaintext_size: manifest.plaintext_size,
                })
                .collect(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn append_attachment_chunk(
        &mut self,
        attachment_id: &str,
        plaintext_offset: u64,
        chunk: &[u8],
    ) -> UploadResult<()> {
        if chunk.len() > MAX_IPC_CHUNK_BYTES {
            return Err(BackupUploadError::SizeLimit);
        }
        let manifest = self
            .manifests
            .get(self.next_attachment)
            .ok_or(BackupUploadError::InvalidAttachmentStream)?;
        if attachment_id != manifest.id
            || plaintext_offset != self.attachment_plaintext.len() as u64
        {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        let next_len = self
            .attachment_plaintext
            .len()
            .checked_add(chunk.len())
            .ok_or(BackupUploadError::SizeLimit)?;
        if next_len as u64 > manifest.plaintext_size || next_len > attachment::MAX_ATTACHMENT_BYTES
        {
            return Err(BackupUploadError::SizeLimit);
        }
        self.attachment_plaintext.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish_attachment(&mut self, attachment_id: &str) -> UploadResult<()> {
        let manifest = self
            .manifests
            .get(self.next_attachment)
            .ok_or(BackupUploadError::InvalidAttachmentStream)?;
        if attachment_id != manifest.id
            || self.attachment_plaintext.len() as u64 != manifest.plaintext_size
        {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        let binding = AttachmentBinding::from_manifest(&self.metadata, manifest);
        let ciphertext =
            attachment::seal_attachment(&self.attachment_plaintext, &binding, &self.master_key)
                .map_err(map_attachment_error);
        self.attachment_plaintext.fill(0);
        self.attachment_plaintext.clear();
        let ciphertext = ciphertext?;
        let object_id =
            attachment_object_id(&self.master_key, &self.metadata.backup_id, &manifest.id)?;
        if self
            .staged
            .iter()
            .any(|item| item.plan.object_id == object_id)
        {
            return Err(BackupUploadError::Encryption);
        }
        let staged = stage_ciphertext(
            &self.staging_dir,
            object_id,
            BackupObjectKind::Attachment,
            &ciphertext,
        )?;
        self.staged.push(staged);
        self.next_attachment += 1;
        Ok(())
    }

    pub fn plan(&self) -> UploadResult<BackupPlan> {
        if self.next_attachment != self.manifests.len() || !self.attachment_plaintext.is_empty() {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        Ok(BackupPlan {
            backup_id: self.metadata.backup_id.clone(),
            generation: self.metadata.device_generation,
            created_at: self.metadata.created_at.clone(),
            protocol_version: backup::PROTOCOL_VERSION,
            objects: self.staged.iter().map(|item| item.plan.clone()).collect(),
        })
    }

    /// Verify every staged byte against the frozen plan before the first
    /// request, then upload main-first / manifest-order and commit last.
    pub fn upload(
        &self,
        transport: &dyn BackupCloudTransport,
        account: &mut AccountSession,
    ) -> UploadResult<BackupUploadOutcome> {
        let plan = self.plan()?;
        for staged in &self.staged {
            verify_staged_object(staged)?;
        }

        transport.create_plan(account, &self.writer_id, &plan)?;
        for staged in &self.staged {
            // Only one opaque object is resident at a time. Recheck after the
            // pre-plan pass so an unexpected local modification cannot turn a
            // declared digest into different uploaded bytes.
            let mut ciphertext = Vec::new();
            File::open(&staged.path)
                .and_then(|mut file| file.read_to_end(&mut ciphertext))
                .map_err(|_| BackupUploadError::Staging)?;
            if ciphertext.len() as u64 != staged.plan.bytes
                || sha256_hex(&ciphertext) != staged.plan.sha256
            {
                return Err(BackupUploadError::Staging);
            }
            transport.put_object(
                account,
                &self.writer_id,
                &plan.backup_id,
                &staged.plan,
                &ciphertext,
            )?;
        }
        let outcome = transport.commit(account, &self.writer_id, &plan)?;
        if outcome.backup_id != plan.backup_id
            || outcome.generation != plan.generation
            || outcome.object_count != plan.objects.len()
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(outcome)
    }
}

fn verify_staged_object(staged: &StagedObject) -> UploadResult<()> {
    let mut file = File::open(&staged.path).map_err(|_| BackupUploadError::Staging)?;
    if file
        .metadata()
        .map_err(|_| BackupUploadError::Staging)?
        .len()
        != staged.plan.bytes
    {
        return Err(BackupUploadError::Staging);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| BackupUploadError::Staging)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if hex(&digest.finalize()) != staged.plan.sha256 {
        return Err(BackupUploadError::Staging);
    }
    Ok(())
}

impl Drop for BackupUploadSession {
    fn drop(&mut self) {
        self.master_key.fill(0);
        self.attachment_plaintext.fill(0);
        let _ = std::fs::remove_dir_all(&self.staging_dir);
    }
}
