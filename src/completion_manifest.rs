//! Immutable completion manifests and their read-only review resolver.
//!
//! This is the adapter boundary for the worker-owned completion protocol. A
//! worker names exact Git or content-addressed outputs; reviewers receive only
//! bytes materialized by this module, never a mutable worker worktree. Missing,
//! inaccessible, oversized, protected, or digest-mismatched data is classified
//! as incomplete evidence before semantic review begins.

use crate::control_plane::assert_tree_has_no_control_plane;
use crate::identity::{blake3_32, canonical_json};
use crate::simple_land::CompletionContract;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use thiserror::Error;

pub const COMPLETION_MANIFEST_VERSION: u32 = 1;
pub const COMPLETION_ARTIFACT_STORE_VERSION: u32 = 1;

/// A BLAKE3 digest of exact bytes, serialized as `b3:<64 lowercase hex>`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("b3:") else {
            return Err("content digest must start with b3:".to_string());
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("content digest must contain 64 lowercase hexadecimal digits".to_string());
        }
        Ok(Self(value))
    }

    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("b3:{}", hex::encode(blake3_32(bytes))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn object_name(&self) -> &str {
        self.0.strip_prefix("b3:").expect("validated digest")
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ContentDigest> for String {
    fn from(value: ContentDigest) -> Self {
        value.0
    }
}

/// An opaque locator accepted by the resolver. Direct filesystem paths and
/// URLs are intentionally not representable: workers must first publish bytes
/// into the content-addressed completion object store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImmutableLocator {
    CompletionObject { digest: ContentDigest },
}

impl ImmutableLocator {
    pub fn digest(&self) -> &ContentDigest {
        match self {
            Self::CompletionObject { digest } => digest,
        }
    }
}

/// Optional bounded bytes used when the exact object is too large to place in a
/// model review context. The resolver always verifies the full source object
/// before accepting its projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewProjection {
    pub content_digest: ContentDigest,
    pub immutable_locator: ImmutableLocator,
    pub media_type: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitOutput {
    pub commit_oid: String,
    pub integrated_main_oid: String,
    pub tree_oid: String,
    pub diff_bundle_digest: ContentDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactOutput {
    pub content_digest: ContentDigest,
    pub immutable_locator: ImmutableLocator,
    pub media_type: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_projection: Option<ReviewProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceRef {
    pub content_digest: ContentDigest,
    pub immutable_locator: ImmutableLocator,
    pub evidence_kind: String,
    pub media_type: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_projection: Option<ReviewProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalOutput {
    pub adapter_kind: String,
    pub resource_id: String,
    pub before_digest: ContentDigest,
    pub after_digest: ContentDigest,
    pub operation_receipt: EvidenceRef,
    pub verification_probe: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputRef {
    Git(GitOutput),
    Artifact(ArtifactOutput),
    External(Box<ExternalOutput>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionManifest {
    pub manifest_version: u32,
    pub task_id: String,
    pub generation: u64,
    pub completion_contract: CompletionContract,
    pub requirements_digest: ContentDigest,
    pub source_revision: String,
    pub outputs: Vec<OutputRef>,
    pub validation_evidence: Vec<EvidenceRef>,
    pub worker_summary_digest: ContentDigest,
}

impl CompletionManifest {
    pub fn digest(&self) -> Result<ContentDigest, ManifestValidationError> {
        Ok(ContentDigest::of_bytes(&self.canonical_bytes()?))
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManifestValidationError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| ManifestValidationError::Invalid(error.to_string()))?;
        Ok(canonical_json(&value))
    }

    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.manifest_version != COMPLETION_MANIFEST_VERSION {
            return Err(ManifestValidationError::Invalid(format!(
                "unsupported manifest version {}",
                self.manifest_version
            )));
        }
        if self.task_id.trim().is_empty() {
            return Err(ManifestValidationError::Invalid(
                "task_id must not be empty".to_string(),
            ));
        }
        if self.source_revision.trim().is_empty() {
            return Err(ManifestValidationError::Invalid(
                "source_revision must not be empty".to_string(),
            ));
        }
        if self.outputs.is_empty() {
            return Err(ManifestValidationError::Invalid(
                "manifest must declare at least one output".to_string(),
            ));
        }
        if self.validation_evidence.is_empty() {
            return Err(ManifestValidationError::Invalid(
                "manifest must declare validation evidence".to_string(),
            ));
        }

        let git_outputs = self
            .outputs
            .iter()
            .filter(|output| matches!(output, OutputRef::Git(_)))
            .count();
        match self.completion_contract {
            CompletionContract::Land if git_outputs != 1 => {
                return Err(ManifestValidationError::Invalid(
                    "Land requires exactly one Git output".to_string(),
                ));
            }
            CompletionContract::Report | CompletionContract::Explore if git_outputs != 0 => {
                return Err(ManifestValidationError::Invalid(
                    "Report and Explore must not declare Git outputs".to_string(),
                ));
            }
            _ => {}
        }

        let mut identities = BTreeSet::new();
        for output in &self.outputs {
            let identity = match output {
                OutputRef::Git(git) => {
                    validate_oid(&git.commit_oid)?;
                    validate_oid(&git.integrated_main_oid)?;
                    validate_oid(&git.tree_oid)?;
                    format!("git:{}", git.commit_oid)
                }
                OutputRef::Artifact(artifact) => {
                    validate_blob_ref(
                        &artifact.content_digest,
                        &artifact.immutable_locator,
                        &artifact.media_type,
                        artifact.size,
                        artifact.review_projection.as_ref(),
                    )?;
                    format!("artifact:{}", artifact.content_digest)
                }
                OutputRef::External(external) => {
                    if external.adapter_kind.trim().is_empty()
                        || external.resource_id.trim().is_empty()
                    {
                        return Err(ManifestValidationError::Invalid(
                            "external output requires adapter_kind and resource_id".to_string(),
                        ));
                    }
                    validate_evidence(&external.operation_receipt)?;
                    validate_evidence(&external.verification_probe)?;
                    format!(
                        "external:{}:{}",
                        external.adapter_kind, external.resource_id
                    )
                }
            };
            if !identities.insert(identity.clone()) {
                return Err(ManifestValidationError::Invalid(format!(
                    "duplicate output identity {identity}"
                )));
            }
        }

        for evidence in &self.validation_evidence {
            validate_evidence(evidence)?;
            let identity = format!(
                "evidence:{}:{}",
                evidence.evidence_kind, evidence.content_digest
            );
            if !identities.insert(identity.clone()) {
                return Err(ManifestValidationError::Invalid(format!(
                    "duplicate evidence identity {identity}"
                )));
            }
        }
        Ok(())
    }
}

fn validate_blob_ref(
    digest: &ContentDigest,
    locator: &ImmutableLocator,
    media_type: &str,
    size: u64,
    projection: Option<&ReviewProjection>,
) -> Result<(), ManifestValidationError> {
    if locator.digest() != digest {
        return Err(ManifestValidationError::Invalid(format!(
            "locator digest {} does not match declared digest {digest}",
            locator.digest()
        )));
    }
    if media_type.trim().is_empty() {
        return Err(ManifestValidationError::Invalid(
            "media_type must not be empty".to_string(),
        ));
    }
    if let Some(projection) = projection {
        if projection.immutable_locator.digest() != &projection.content_digest {
            return Err(ManifestValidationError::Invalid(
                "projection locator does not match its digest".to_string(),
            ));
        }
        if projection.media_type.trim().is_empty() {
            return Err(ManifestValidationError::Invalid(
                "projection media_type must not be empty".to_string(),
            ));
        }
        if projection.size > size {
            return Err(ManifestValidationError::Invalid(
                "review projection cannot be larger than its source".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_evidence(evidence: &EvidenceRef) -> Result<(), ManifestValidationError> {
    if evidence.evidence_kind.trim().is_empty() {
        return Err(ManifestValidationError::Invalid(
            "evidence_kind must not be empty".to_string(),
        ));
    }
    validate_blob_ref(
        &evidence.content_digest,
        &evidence.immutable_locator,
        &evidence.media_type,
        evidence.size,
        evidence.review_projection.as_ref(),
    )
}

fn validate_oid(oid: &str) -> Result<(), ManifestValidationError> {
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestValidationError::Invalid(format!(
            "invalid Git object id {oid:?}"
        )));
    }
    Ok(())
}

/// Immutable handle produced when a worker submits its manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionManifestRef {
    pub content_digest: ContentDigest,
    pub immutable_locator: ImmutableLocator,
    pub size: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ManifestValidationError {
    #[error("invalid completion manifest: {0}")]
    Invalid(String),
}

/// Content-addressed store used by Report/Explore outputs and evidence.
/// Objects are create-once and are rehashed on every resolution.
#[derive(Clone, Debug)]
pub struct CompletionArtifactStore {
    root: PathBuf,
}

impl CompletionArtifactStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactStoreError> {
        let root = root.into();
        reject_symlink_if_present(&root)?;
        fs::create_dir_all(&root).map_err(ArtifactStoreError::Io)?;
        let objects = root.join("objects");
        reject_symlink_if_present(&objects)?;
        fs::create_dir_all(&objects).map_err(ArtifactStoreError::Io)?;
        sync_dir(&root).map_err(ArtifactStoreError::Io)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        media_type: impl Into<String>,
    ) -> Result<ArtifactOutput, ArtifactStoreError> {
        let media_type = media_type.into();
        if media_type.trim().is_empty() {
            return Err(ArtifactStoreError::InvalidMediaType);
        }
        let digest = ContentDigest::of_bytes(bytes);
        let path = self.object_path(&digest);
        if path.exists() {
            verify_file(&path, &digest, bytes.len() as u64)
                .map_err(ArtifactStoreError::ExistingObject)?;
        } else {
            let temporary = self.root.join(format!(
                ".tmp-{}-{}",
                digest.object_name(),
                uuid::Uuid::now_v7()
            ));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(ArtifactStoreError::Io)?;
            file.write_all(bytes).map_err(ArtifactStoreError::Io)?;
            file.sync_all().map_err(ArtifactStoreError::Io)?;
            // A hard-link is a create-if-absent publication primitive: unlike
            // rename it cannot replace an object another writer just created.
            match fs::hard_link(&temporary, &path) {
                Ok(()) => {
                    fs::remove_file(&temporary).map_err(ArtifactStoreError::Io)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let _ = fs::remove_file(&temporary);
                    verify_file(&path, &digest, bytes.len() as u64)
                        .map_err(ArtifactStoreError::ExistingObject)?;
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    return Err(ArtifactStoreError::Io(error));
                }
            }
            sync_dir(path.parent().expect("object path has parent"))
                .map_err(ArtifactStoreError::Io)?;
        }
        Ok(ArtifactOutput {
            content_digest: digest.clone(),
            immutable_locator: ImmutableLocator::CompletionObject { digest },
            media_type,
            size: bytes.len() as u64,
            review_projection: None,
        })
    }

    /// Snapshot one regular file into the immutable store. The returned
    /// locator never retains or reopens the mutable source path.
    pub fn put_file(
        &self,
        source: &Path,
        media_type: impl Into<String>,
    ) -> Result<ArtifactOutput, ArtifactStoreError> {
        let media_type = media_type.into();
        if media_type.trim().is_empty() {
            return Err(ArtifactStoreError::InvalidMediaType);
        }
        let path_metadata = fs::symlink_metadata(source).map_err(ArtifactStoreError::Io)?;
        if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
            return Err(ArtifactStoreError::InvalidSource(source.to_path_buf()));
        }
        let mut input = File::open(source).map_err(ArtifactStoreError::Io)?;
        let opened_metadata = input.metadata().map_err(ArtifactStoreError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if path_metadata.dev() != opened_metadata.dev()
                || path_metadata.ino() != opened_metadata.ino()
            {
                return Err(ArtifactStoreError::InvalidSource(source.to_path_buf()));
            }
        }

        let temporary = self
            .root
            .join(format!(".tmp-ingest-{}", uuid::Uuid::now_v7()));
        let ingest = (|| -> Result<(ContentDigest, u64), ArtifactStoreError> {
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(ArtifactStoreError::Io)?;
            let mut hasher = blake3::Hasher::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = input.read(&mut buffer).map_err(ArtifactStoreError::Io)?;
                if count == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(ArtifactStoreError::Io)?;
                hasher.update(&buffer[..count]);
                size = size
                    .checked_add(count as u64)
                    .ok_or(ArtifactStoreError::SourceTooLarge)?;
            }
            output.sync_all().map_err(ArtifactStoreError::Io)?;
            let digest = ContentDigest::parse(format!("b3:{}", hasher.finalize().to_hex()))
                .expect("BLAKE3 produces a valid digest");
            Ok((digest, size))
        })();
        let (digest, size) = match ingest {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let destination = self.object_path(&digest);
        match fs::hard_link(&temporary, &destination) {
            Ok(()) => {
                fs::remove_file(&temporary).map_err(ArtifactStoreError::Io)?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temporary);
                verify_file(&destination, &digest, size)
                    .map_err(ArtifactStoreError::ExistingObject)?;
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(ArtifactStoreError::Io(error));
            }
        }
        sync_dir(destination.parent().expect("object path has parent"))
            .map_err(ArtifactStoreError::Io)?;
        Ok(ArtifactOutput {
            content_digest: digest.clone(),
            immutable_locator: ImmutableLocator::CompletionObject { digest },
            media_type,
            size,
            review_projection: None,
        })
    }

    pub fn put_manifest(
        &self,
        manifest: &CompletionManifest,
    ) -> Result<CompletionManifestRef, ArtifactStoreError> {
        let bytes = manifest
            .canonical_bytes()
            .map_err(|error| ArtifactStoreError::InvalidManifest(error.to_string()))?;
        let expected = manifest
            .digest()
            .map_err(|error| ArtifactStoreError::InvalidManifest(error.to_string()))?;
        let artifact = self.put_bytes(&bytes, "application/vnd.worksgood.completion+json")?;
        if artifact.content_digest != expected {
            return Err(ArtifactStoreError::InvalidManifest(
                "canonical manifest digest disagreed during publication".to_string(),
            ));
        }
        Ok(CompletionManifestRef {
            content_digest: artifact.content_digest,
            immutable_locator: artifact.immutable_locator,
            size: artifact.size,
        })
    }

    /// Read and re-hash one immutable artifact through a single descriptor.
    /// Callers use this for requirements, summaries and review receipts; it
    /// never reopens a mutable source path.
    pub fn read_artifact(
        &self,
        artifact: &ArtifactOutput,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        if artifact.immutable_locator.digest() != &artifact.content_digest {
            return Err(ArtifactStoreError::InvalidObject(
                "artifact locator does not match its declared digest".to_string(),
            ));
        }
        if artifact.size > max_bytes {
            return Err(ArtifactStoreError::InvalidObject(format!(
                "artifact size {} exceeds read limit {}",
                artifact.size, max_bytes
            )));
        }
        verify_and_read_object(
            &self.object_path(&artifact.content_digest),
            &artifact.content_digest,
            artifact.size,
            "completion artifact",
            true,
        )
        .map_err(|error| ArtifactStoreError::InvalidObject(error.to_string()))?
        .ok_or_else(|| ArtifactStoreError::InvalidObject("artifact bytes unavailable".to_string()))
    }

    pub fn read_manifest(
        &self,
        manifest_ref: &CompletionManifestRef,
        max_bytes: u64,
    ) -> Result<CompletionManifest, ArtifactStoreError> {
        let artifact = ArtifactOutput {
            content_digest: manifest_ref.content_digest.clone(),
            immutable_locator: manifest_ref.immutable_locator.clone(),
            media_type: "application/vnd.worksgood.completion+json".to_string(),
            size: manifest_ref.size,
            review_projection: None,
        };
        let bytes = self.read_artifact(&artifact, max_bytes)?;
        let manifest: CompletionManifest = serde_json::from_slice(&bytes)
            .map_err(|error| ArtifactStoreError::InvalidObject(error.to_string()))?;
        let canonical = manifest
            .canonical_bytes()
            .map_err(|error| ArtifactStoreError::InvalidObject(error.to_string()))?;
        if canonical != bytes
            || manifest.digest().ok().as_ref() != Some(&manifest_ref.content_digest)
        {
            return Err(ArtifactStoreError::InvalidObject(
                "stored manifest is not the canonical manifest named by its reference".to_string(),
            ));
        }
        Ok(manifest)
    }

    pub fn evidence_from_bytes(
        &self,
        bytes: &[u8],
        evidence_kind: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Result<EvidenceRef, ArtifactStoreError> {
        let artifact = self.put_bytes(bytes, media_type)?;
        let evidence_kind = evidence_kind.into();
        if evidence_kind.trim().is_empty() {
            return Err(ArtifactStoreError::InvalidEvidenceKind);
        }
        Ok(EvidenceRef {
            content_digest: artifact.content_digest,
            immutable_locator: artifact.immutable_locator,
            evidence_kind,
            media_type: artifact.media_type,
            size: artifact.size,
            review_projection: None,
        })
    }

    fn object_path(&self, digest: &ContentDigest) -> PathBuf {
        self.root.join("objects").join(digest.object_name())
    }
}

#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    #[error("artifact store I/O error: {0}")]
    Io(#[source] io::Error),
    #[error("artifact store path must not be a symbolic link: {0}")]
    Symlink(PathBuf),
    #[error("existing immutable object is invalid: {0}")]
    ExistingObject(String),
    #[error("immutable object reference is invalid: {0}")]
    InvalidObject(String),
    #[error("media type must not be empty")]
    InvalidMediaType,
    #[error("evidence kind must not be empty")]
    InvalidEvidenceKind,
    #[error("artifact source must be a regular non-symlink file: {0}")]
    InvalidSource(PathBuf),
    #[error("artifact source is too large")]
    SourceTooLarge,
    #[error("invalid completion manifest: {0}")]
    InvalidManifest(String),
}

fn reject_symlink_if_present(path: &Path) -> Result<(), ArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ArtifactStoreError::Symlink(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArtifactStoreError::Io(error)),
    }
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvePolicy {
    pub max_item_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ResolvePolicy {
    fn default() -> Self {
        Self {
            max_item_bytes: 4 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteEvidenceKind {
    InvalidManifest,
    Missing,
    Inaccessible,
    DigestMismatch,
    SizeMismatch,
    OversizedWithoutProjection,
    ProtectedControlPlane,
    GitObjectMismatch,
    UnsupportedExternalAdapter,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("incomplete evidence ({kind:?}) for {reference}: {detail}")]
pub struct IncompleteEvidence {
    pub kind: IncompleteEvidenceKind,
    pub reference: String,
    pub detail: String,
}

impl IncompleteEvidence {
    fn new(
        kind: IncompleteEvidenceKind,
        reference: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            reference: reference.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPayload {
    pub label: String,
    pub source_digest: ContentDigest,
    pub inspected_digest: ContentDigest,
    pub media_type: String,
    pub source_size: u64,
    pub projected: bool,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedOutput {
    Git {
        commit_oid: String,
        tree_oid: String,
        diff: ResolvedPayload,
    },
    Artifact(ResolvedPayload),
    External {
        adapter_kind: String,
        resource_id: String,
        operation_receipt: ResolvedPayload,
        verification_probe: ResolvedPayload,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEvidence {
    pub evidence_kind: String,
    pub payload: ResolvedPayload,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedReviewBundle {
    pub manifest_digest: ContentDigest,
    pub requirements_digest: ContentDigest,
    pub manifest_bytes: Vec<u8>,
    pub requirements_bytes: Vec<u8>,
    pub worker_summary_bytes: Vec<u8>,
    pub dependency_outputs: Vec<ResolvedEvidence>,
    pub outputs: Vec<ResolvedOutput>,
    pub validation_evidence: Vec<ResolvedEvidence>,
    /// Git object IDs and BLAKE3 content digests verified while resolving.
    pub inspected_output_digests: Vec<String>,
}

pub struct ExternalVerification {
    pub after_digest: ContentDigest,
    pub probe_bytes: Vec<u8>,
}

/// External adapters expose only a read-only verification operation. They do
/// not receive a worker worktree and cannot mutate the named resource.
pub trait ExternalOutputVerifier {
    fn verify_read_only(
        &self,
        output: &ExternalOutput,
    ) -> Result<ExternalVerification, IncompleteEvidence>;
}

pub struct ReviewResolver<'a> {
    artifact_store: &'a CompletionArtifactStore,
    repository: Option<&'a Path>,
    external_verifier: Option<&'a dyn ExternalOutputVerifier>,
    policy: ResolvePolicy,
}

impl<'a> ReviewResolver<'a> {
    pub fn new(artifact_store: &'a CompletionArtifactStore) -> Self {
        Self {
            artifact_store,
            repository: None,
            external_verifier: None,
            policy: ResolvePolicy::default(),
        }
    }

    pub fn repository(mut self, repository: &'a Path) -> Self {
        self.repository = Some(repository);
        self
    }

    pub fn external_verifier(mut self, verifier: &'a dyn ExternalOutputVerifier) -> Self {
        self.external_verifier = Some(verifier);
        self
    }

    pub fn policy(mut self, policy: ResolvePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Load an immutable submitted manifest, verify its content identity, then
    /// materialize the review bundle. Production callers should prefer this to
    /// passing an in-memory manifest value directly.
    pub fn resolve_submission(
        &self,
        submission: &CompletionManifestRef,
        requirements_bytes: &[u8],
        worker_summary_bytes: &[u8],
        dependency_outputs: &[EvidenceRef],
    ) -> Result<ResolvedReviewBundle, IncompleteEvidence> {
        if submission.immutable_locator.digest() != &submission.content_digest {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest submission",
                "manifest locator does not match its declared digest",
            ));
        }
        if submission.size > self.policy.max_item_bytes {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::OversizedWithoutProjection,
                "manifest submission",
                format!("{} bytes exceeds manifest review limit", submission.size),
            ));
        }
        let path = self.artifact_store.object_path(&submission.content_digest);
        let bytes = verify_and_read_object(
            &path,
            &submission.content_digest,
            submission.size,
            "manifest submission",
            true,
        )?
        .expect("read requested");
        let manifest: CompletionManifest = serde_json::from_slice(&bytes).map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest submission",
                error.to_string(),
            )
        })?;
        let canonical = manifest.canonical_bytes().map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest submission",
                error.to_string(),
            )
        })?;
        if canonical != bytes || manifest.digest().ok().as_ref() != Some(&submission.content_digest)
        {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::DigestMismatch,
                "manifest submission",
                "stored manifest is not the exact canonical manifest named by the submission",
            ));
        }
        self.resolve_with_dependencies(
            &manifest,
            requirements_bytes,
            worker_summary_bytes,
            dependency_outputs,
        )
    }

    /// Resolve a manifest value already obtained from a trusted immutable
    /// submission. Tests and higher-level persistence adapters may use this
    /// directly; it still verifies every referenced byte.
    pub fn resolve(
        &self,
        manifest: &CompletionManifest,
        requirements_bytes: &[u8],
        worker_summary_bytes: &[u8],
    ) -> Result<ResolvedReviewBundle, IncompleteEvidence> {
        self.resolve_with_dependencies(manifest, requirements_bytes, worker_summary_bytes, &[])
    }

    /// Resolve graph-supplied dependency outputs together with the worker's
    /// manifest. Dependencies are not worker-editable manifest fields; the
    /// caller supplies the exact graph-authoritative references.
    pub fn resolve_with_dependencies(
        &self,
        manifest: &CompletionManifest,
        requirements_bytes: &[u8],
        worker_summary_bytes: &[u8],
        dependency_outputs: &[EvidenceRef],
    ) -> Result<ResolvedReviewBundle, IncompleteEvidence> {
        manifest.validate().map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest",
                error.to_string(),
            )
        })?;
        verify_inline(
            "requirements",
            requirements_bytes,
            &manifest.requirements_digest,
        )?;
        verify_inline(
            "worker_summary",
            worker_summary_bytes,
            &manifest.worker_summary_digest,
        )?;

        let manifest_digest = manifest.digest().map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest",
                error.to_string(),
            )
        })?;
        let manifest_bytes = manifest.canonical_bytes().map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                "manifest",
                error.to_string(),
            )
        })?;

        let mut budget = ResolutionBudget::new(self.policy);
        budget.include_inline("completion manifest", manifest_bytes.len() as u64)?;
        budget.include_inline("requirements", requirements_bytes.len() as u64)?;
        budget.include_inline("worker summary", worker_summary_bytes.len() as u64)?;
        let mut outputs = Vec::with_capacity(manifest.outputs.len());
        let mut inspected_output_digests = Vec::new();
        for output in &manifest.outputs {
            match output {
                OutputRef::Git(git) => {
                    let repository = self.repository.ok_or_else(|| {
                        IncompleteEvidence::new(
                            IncompleteEvidenceKind::Inaccessible,
                            git.commit_oid.clone(),
                            "Land manifest requires an explicit read-only repository",
                        )
                    })?;
                    let resolved = resolve_git(repository, git, &mut budget)?;
                    inspected_output_digests.push(git.commit_oid.clone());
                    inspected_output_digests.push(git.tree_oid.clone());
                    inspected_output_digests.push(git.diff_bundle_digest.to_string());
                    outputs.push(resolved);
                }
                OutputRef::Artifact(artifact) => {
                    let payload = self.resolve_artifact(artifact, &mut budget)?;
                    inspected_output_digests.push(artifact.content_digest.to_string());
                    outputs.push(ResolvedOutput::Artifact(payload));
                }
                OutputRef::External(external) => {
                    let verifier = self.external_verifier.ok_or_else(|| {
                        IncompleteEvidence::new(
                            IncompleteEvidenceKind::UnsupportedExternalAdapter,
                            format!("{}:{}", external.adapter_kind, external.resource_id),
                            "no exact external verification adapter was supplied",
                        )
                    })?;
                    let receipt = self.resolve_evidence_payload(
                        &external.operation_receipt,
                        "external operation receipt",
                        &mut budget,
                    )?;
                    let stored_probe = self.resolve_evidence_payload(
                        &external.verification_probe,
                        "external verification probe",
                        &mut budget,
                    )?;
                    let observed = verifier.verify_read_only(external)?;
                    if observed.after_digest != external.after_digest {
                        return Err(IncompleteEvidence::new(
                            IncompleteEvidenceKind::DigestMismatch,
                            format!("{}:{}", external.adapter_kind, external.resource_id),
                            format!(
                                "read-only probe observed {}, expected {}",
                                observed.after_digest, external.after_digest
                            ),
                        ));
                    }
                    verify_inline(
                        "external verification probe",
                        &observed.probe_bytes,
                        &external.verification_probe.content_digest,
                    )?;
                    if observed.probe_bytes != stored_probe.bytes {
                        return Err(IncompleteEvidence::new(
                            IncompleteEvidenceKind::DigestMismatch,
                            "external verification probe",
                            "live read-only probe differs from the stored reviewed probe",
                        ));
                    }
                    inspected_output_digests.push(external.after_digest.to_string());
                    outputs.push(ResolvedOutput::External {
                        adapter_kind: external.adapter_kind.clone(),
                        resource_id: external.resource_id.clone(),
                        operation_receipt: receipt,
                        verification_probe: stored_probe,
                    });
                }
            }
        }

        let mut resolved_dependencies = Vec::with_capacity(dependency_outputs.len());
        for dependency in dependency_outputs {
            validate_evidence(dependency).map_err(|error| {
                IncompleteEvidence::new(
                    IncompleteEvidenceKind::InvalidManifest,
                    "dependency output",
                    error.to_string(),
                )
            })?;
            resolved_dependencies.push(ResolvedEvidence {
                evidence_kind: dependency.evidence_kind.clone(),
                payload: self.resolve_evidence_payload(
                    dependency,
                    &format!("dependency output {}", dependency.evidence_kind),
                    &mut budget,
                )?,
            });
        }

        let mut validation_evidence = Vec::with_capacity(manifest.validation_evidence.len());
        for evidence in &manifest.validation_evidence {
            validation_evidence.push(ResolvedEvidence {
                evidence_kind: evidence.evidence_kind.clone(),
                payload: self.resolve_evidence_payload(
                    evidence,
                    &format!("validation evidence {}", evidence.evidence_kind),
                    &mut budget,
                )?,
            });
        }

        Ok(ResolvedReviewBundle {
            manifest_digest,
            requirements_digest: manifest.requirements_digest.clone(),
            manifest_bytes,
            requirements_bytes: requirements_bytes.to_vec(),
            worker_summary_bytes: worker_summary_bytes.to_vec(),
            dependency_outputs: resolved_dependencies,
            outputs,
            validation_evidence,
            inspected_output_digests,
        })
    }

    fn resolve_artifact(
        &self,
        artifact: &ArtifactOutput,
        budget: &mut ResolutionBudget,
    ) -> Result<ResolvedPayload, IncompleteEvidence> {
        self.resolve_blob(
            "artifact output",
            &artifact.content_digest,
            &artifact.immutable_locator,
            &artifact.media_type,
            artifact.size,
            artifact.review_projection.as_ref(),
            budget,
        )
    }

    fn resolve_evidence_payload(
        &self,
        evidence: &EvidenceRef,
        label: &str,
        budget: &mut ResolutionBudget,
    ) -> Result<ResolvedPayload, IncompleteEvidence> {
        self.resolve_blob(
            label,
            &evidence.content_digest,
            &evidence.immutable_locator,
            &evidence.media_type,
            evidence.size,
            evidence.review_projection.as_ref(),
            budget,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_blob(
        &self,
        label: &str,
        digest: &ContentDigest,
        locator: &ImmutableLocator,
        media_type: &str,
        size: u64,
        projection: Option<&ReviewProjection>,
        budget: &mut ResolutionBudget,
    ) -> Result<ResolvedPayload, IncompleteEvidence> {
        if locator.digest() != digest {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::InvalidManifest,
                label,
                "locator digest does not match source digest",
            ));
        }
        let path = self.artifact_store.object_path(digest);

        if budget.can_include(size) {
            let bytes =
                verify_and_read_object(&path, digest, size, label, true)?.expect("read requested");
            budget.include(size);
            return Ok(ResolvedPayload {
                label: label.to_string(),
                source_digest: digest.clone(),
                inspected_digest: digest.clone(),
                media_type: media_type.to_string(),
                source_size: size,
                projected: false,
                bytes,
            });
        }

        // Even when the reviewer sees a projection, the full source object is
        // streamed and digest-verified first.
        verify_and_read_object(&path, digest, size, label, false)?;
        let projection = projection.ok_or_else(|| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::OversizedWithoutProjection,
                label,
                format!(
                    "{size} bytes exceeds item/remaining bundle limits ({}/{})",
                    self.policy.max_item_bytes,
                    budget.remaining()
                ),
            )
        })?;
        let projection_path = self.artifact_store.object_path(&projection.content_digest);
        if !budget.can_include(projection.size) {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::OversizedWithoutProjection,
                format!("{label} projection"),
                format!(
                    "projection is {} bytes, exceeding item/remaining bundle limits",
                    projection.size
                ),
            ));
        }
        let bytes = verify_and_read_object(
            &projection_path,
            &projection.content_digest,
            projection.size,
            &format!("{label} projection"),
            true,
        )?
        .expect("read requested");
        budget.include(projection.size);
        Ok(ResolvedPayload {
            label: label.to_string(),
            source_digest: digest.clone(),
            inspected_digest: projection.content_digest.clone(),
            media_type: projection.media_type.clone(),
            source_size: size,
            projected: true,
            bytes,
        })
    }
}

struct ResolutionBudget {
    policy: ResolvePolicy,
    used: u64,
}

impl ResolutionBudget {
    fn new(policy: ResolvePolicy) -> Self {
        Self { policy, used: 0 }
    }

    fn remaining(&self) -> u64 {
        self.policy.max_total_bytes.saturating_sub(self.used)
    }

    fn can_include(&self, size: u64) -> bool {
        size <= self.policy.max_item_bytes && size <= self.remaining()
    }

    fn include(&mut self, size: u64) {
        self.used = self.used.saturating_add(size);
    }

    fn include_inline(&mut self, label: &str, size: u64) -> Result<(), IncompleteEvidence> {
        if !self.can_include(size) {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::OversizedWithoutProjection,
                label,
                format!("{size} bytes exceeds item/remaining review bundle limits"),
            ));
        }
        self.include(size);
        Ok(())
    }
}

fn verify_inline(
    label: &str,
    bytes: &[u8],
    expected: &ContentDigest,
) -> Result<(), IncompleteEvidence> {
    let observed = ContentDigest::of_bytes(bytes);
    if &observed != expected {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::DigestMismatch,
            label,
            format!("observed {observed}, expected {expected}"),
        ));
    }
    Ok(())
}

fn verify_file(path: &Path, digest: &ContentDigest, size: u64) -> Result<(), String> {
    verify_and_read_object(path, digest, size, "immutable object", false)
        .map(|_| ())
        .map_err(|error| error.detail)
}

/// Open once, verify the exact bytes from that descriptor, and optionally retain
/// them. This avoids a verify-then-reopen TOCTOU window in the review resolver.
fn verify_and_read_object(
    path: &Path,
    digest: &ContentDigest,
    size: u64,
    label: &str,
    retain: bool,
) -> Result<Option<Vec<u8>>, IncompleteEvidence> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        IncompleteEvidence::new(
            if error.kind() == io::ErrorKind::NotFound {
                IncompleteEvidenceKind::Missing
            } else {
                IncompleteEvidenceKind::Inaccessible
            },
            label,
            error.to_string(),
        )
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            label,
            "immutable object is not a regular file",
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            label,
            error.to_string(),
        )
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            label,
            error.to_string(),
        )
    })?;
    if !opened_metadata.is_file() {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            label,
            "opened immutable object is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != opened_metadata.dev()
            || path_metadata.ino() != opened_metadata.ino()
        {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::Inaccessible,
                label,
                "immutable object changed while it was opened",
            ));
        }
    }
    if opened_metadata.len() != size {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::SizeMismatch,
            label,
            format!("observed size {}, expected {size}", opened_metadata.len()),
        ));
    }

    let mut retained = if retain {
        let capacity = usize::try_from(size).map_err(|_| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::OversizedWithoutProjection,
                label,
                "object size does not fit in memory on this platform",
            )
        })?;
        Some(Vec::with_capacity(capacity))
    } else {
        None
    };
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::Inaccessible,
                label,
                error.to_string(),
            )
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        if let Some(bytes) = &mut retained {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    let observed = ContentDigest::parse(format!("b3:{}", hasher.finalize().to_hex()))
        .expect("BLAKE3 produces a valid digest");
    if &observed != digest {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::DigestMismatch,
            label,
            format!("observed digest {observed}, expected {digest}"),
        ));
    }
    Ok(retained)
}

fn resolve_git(
    repository: &Path,
    git: &GitOutput,
    budget: &mut ResolutionBudget,
) -> Result<ResolvedOutput, IncompleteEvidence> {
    validate_oid(&git.commit_oid).map_err(|error| {
        IncompleteEvidence::new(
            IncompleteEvidenceKind::InvalidManifest,
            "Git output",
            error.to_string(),
        )
    })?;
    for oid in [&git.commit_oid, &git.integrated_main_oid] {
        let object = format!("{oid}^{{commit}}");
        let output = git_output(repository, &["cat-file", "-e", &object])?;
        if !output.status.success() {
            return Err(IncompleteEvidence::new(
                IncompleteEvidenceKind::Missing,
                oid.clone(),
                git_stderr(&output),
            ));
        }
    }

    let observed_tree = git_text(
        repository,
        &["rev-parse", &format!("{}^{{tree}}", git.commit_oid)],
    )?;
    if observed_tree != git.tree_oid {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::GitObjectMismatch,
            git.commit_oid.clone(),
            format!(
                "commit tree is {observed_tree}, manifest declares {}",
                git.tree_oid
            ),
        ));
    }
    let ancestry = git_output(
        repository,
        &[
            "merge-base",
            "--is-ancestor",
            &git.integrated_main_oid,
            &git.commit_oid,
        ],
    )?;
    if !ancestry.status.success() {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::GitObjectMismatch,
            git.commit_oid.clone(),
            "candidate does not contain integrated_main_oid",
        ));
    }
    assert_tree_has_no_control_plane(repository, &git.commit_oid).map_err(|error| {
        IncompleteEvidence::new(
            IncompleteEvidenceKind::ProtectedControlPlane,
            git.commit_oid.clone(),
            error.to_string(),
        )
    })?;

    let diff = git_output(
        repository,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            &git.integrated_main_oid,
            &git.commit_oid,
            "--",
        ],
    )?;
    if !diff.status.success() {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            git.commit_oid.clone(),
            git_stderr(&diff),
        ));
    }
    let observed_digest = ContentDigest::of_bytes(&diff.stdout);
    if observed_digest != git.diff_bundle_digest {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::DigestMismatch,
            "Git diff bundle",
            format!(
                "observed {observed_digest}, expected {}",
                git.diff_bundle_digest
            ),
        ));
    }
    let size = diff.stdout.len() as u64;
    if !budget.can_include(size) {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::OversizedWithoutProjection,
            "Git diff bundle",
            format!("{size} bytes exceeds review bundle limits"),
        ));
    }
    budget.include(size);
    Ok(ResolvedOutput::Git {
        commit_oid: git.commit_oid.clone(),
        tree_oid: git.tree_oid.clone(),
        diff: ResolvedPayload {
            label: "Git diff bundle".to_string(),
            source_digest: git.diff_bundle_digest.clone(),
            inspected_digest: git.diff_bundle_digest.clone(),
            media_type: "text/x-diff".to_string(),
            source_size: size,
            projected: false,
            bytes: diff.stdout,
        },
    })
}

fn git_output(repository: &Path, args: &[&str]) -> Result<Output, IncompleteEvidence> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::Inaccessible,
                repository.display().to_string(),
                error.to_string(),
            )
        })
}

fn git_text(repository: &Path, args: &[&str]) -> Result<String, IncompleteEvidence> {
    let output = git_output(repository, args)?;
    if !output.status.success() {
        return Err(IncompleteEvidence::new(
            IncompleteEvidenceKind::Inaccessible,
            repository.display().to_string(),
            git_stderr(&output),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|error| {
            IncompleteEvidence::new(
                IncompleteEvidenceKind::Inaccessible,
                repository.display().to_string(),
                error.to_string(),
            )
        })
}

fn git_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}
