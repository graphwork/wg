use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use worksgood::completion_manifest::{
    ArtifactOutput, COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest,
    ContentDigest, EvidenceRef, ExternalOutput, GitOutput, ImmutableLocator,
    IncompleteEvidenceKind, OutputRef, ResolvePolicy, ReviewProjection, ReviewResolver,
};
use worksgood::simple_land::CompletionContract;

const REQUIREMENTS: &[u8] = b"produce the exact reviewed output";
const SUMMARY: &[u8] = b"implemented and validated the requested output";

fn evidence(store: &CompletionArtifactStore) -> EvidenceRef {
    store
        .evidence_from_bytes(b"validation passed\n", "test-log", "text/plain")
        .unwrap()
}

fn report_manifest(output: ArtifactOutput, evidence: EvidenceRef) -> CompletionManifest {
    CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: "report-task".to_string(),
        generation: 1,
        completion_contract: CompletionContract::Report,
        requirements_digest: ContentDigest::of_bytes(REQUIREMENTS),
        source_revision: "main@reference".to_string(),
        outputs: vec![OutputRef::Artifact(output)],
        validation_evidence: vec![evidence],
        worker_summary_digest: ContentDigest::of_bytes(SUMMARY),
    }
}

fn object_path(store: &CompletionArtifactStore, digest: &ContentDigest) -> std::path::PathBuf {
    store
        .root()
        .join("objects")
        .join(digest.as_str().strip_prefix("b3:").unwrap())
}

#[test]
fn report_resolves_only_digest_verified_store_objects() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let output = store
        .put_bytes(b"immutable report\n", "text/markdown")
        .unwrap();
    let expected_digest = output.content_digest.clone();
    let manifest = report_manifest(output, evidence(&store));
    let submission = store.put_manifest(&manifest).unwrap();

    let bundle = ReviewResolver::new(&store)
        .resolve_submission(&submission, REQUIREMENTS, SUMMARY, &[])
        .unwrap();

    assert_eq!(bundle.manifest_digest, manifest.digest().unwrap());
    assert_eq!(
        bundle.inspected_output_digests,
        vec![expected_digest.to_string()]
    );
    assert_eq!(bundle.outputs.len(), 1);
    assert_eq!(bundle.validation_evidence.len(), 1);
}

#[test]
fn mutable_source_file_is_snapshotted_not_retained_as_a_locator() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let source = temp.path().join("report.md");
    fs::write(&source, "reviewed version\n").unwrap();
    let output = store.put_file(&source, "text/markdown").unwrap();
    fs::write(&source, "later mutable version\n").unwrap();
    let manifest = report_manifest(output, evidence(&store));

    let bundle = ReviewResolver::new(&store)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap();
    let worksgood::completion_manifest::ResolvedOutput::Artifact(payload) = &bundle.outputs[0]
    else {
        panic!("expected artifact output")
    };
    assert_eq!(payload.bytes, b"reviewed version\n");
}

#[test]
fn graph_supplied_dependency_outputs_are_resolved_by_digest() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let output = store.put_bytes(b"report", "text/plain").unwrap();
    let dependency = store
        .evidence_from_bytes(
            b"exact predecessor result",
            "dependency:prepare-data",
            "text/plain",
        )
        .unwrap();
    let manifest = report_manifest(output, evidence(&store));

    let bundle = ReviewResolver::new(&store)
        .resolve_with_dependencies(&manifest, REQUIREMENTS, SUMMARY, &[dependency])
        .unwrap();

    assert_eq!(bundle.dependency_outputs.len(), 1);
    assert_eq!(
        bundle.dependency_outputs[0].payload.bytes,
        b"exact predecessor result"
    );
}

#[test]
fn changed_requirements_are_incomplete_evidence_not_semantic_review() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let output = store.put_bytes(b"report", "text/plain").unwrap();
    let manifest = report_manifest(output, evidence(&store));

    let error = ReviewResolver::new(&store)
        .resolve(&manifest, b"changed requirements", SUMMARY)
        .unwrap_err();

    assert_eq!(error.kind, IncompleteEvidenceKind::DigestMismatch);
    assert_eq!(error.reference, "requirements");
}

#[test]
fn missing_and_mutated_objects_fail_closed_before_review() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let output = store.put_bytes(b"original", "text/plain").unwrap();
    let path = object_path(&store, &output.content_digest);
    let manifest = report_manifest(output, evidence(&store));

    fs::write(&path, b"tampered").unwrap();
    let error = ReviewResolver::new(&store)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(error.kind, IncompleteEvidenceKind::DigestMismatch);

    fs::remove_file(path).unwrap();
    let error = ReviewResolver::new(&store)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(error.kind, IncompleteEvidenceKind::Missing);
}

#[test]
fn oversized_source_requires_and_verifies_a_bounded_projection() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let source = vec![b'x'; 16 * 1024];
    let mut output = store
        .put_bytes(&source, "application/octet-stream")
        .unwrap();
    let validation = evidence(&store);
    let policy = ResolvePolicy {
        max_item_bytes: 4 * 1024,
        max_total_bytes: 32 * 1024,
    };
    let manifest = report_manifest(output.clone(), validation.clone());

    let error = ReviewResolver::new(&store)
        .policy(policy)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(
        error.kind,
        IncompleteEvidenceKind::OversizedWithoutProjection
    );

    let projection = store
        .put_bytes(b"bounded projection of large binary", "text/plain")
        .unwrap();
    output.review_projection = Some(ReviewProjection {
        content_digest: projection.content_digest.clone(),
        immutable_locator: projection.immutable_locator,
        media_type: projection.media_type,
        size: projection.size,
    });
    let manifest = report_manifest(output, validation);
    let bundle = ReviewResolver::new(&store)
        .policy(policy)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap();

    let worksgood::completion_manifest::ResolvedOutput::Artifact(payload) = &bundle.outputs[0]
    else {
        panic!("expected artifact output")
    };
    assert!(payload.projected);
    assert_eq!(payload.bytes, b"bounded projection of large binary");
}

#[cfg(unix)]
#[test]
fn symlinked_store_objects_are_inaccessible_evidence() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let output = store.put_bytes(b"review me", "text/plain").unwrap();
    let path = object_path(&store, &output.content_digest);
    let outside = temp.path().join("outside");
    fs::write(&outside, b"review me").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    let manifest = report_manifest(output, evidence(&store));

    let error = ReviewResolver::new(&store)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(error.kind, IncompleteEvidenceKind::Inaccessible);
}

#[test]
fn locator_cannot_name_a_different_object() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let mut output = store.put_bytes(b"one", "text/plain").unwrap();
    let other = store.put_bytes(b"two", "text/plain").unwrap();
    output.immutable_locator = other.immutable_locator;
    let manifest = report_manifest(output, evidence(&store));

    let error = manifest.validate().unwrap_err();
    assert!(error.to_string().contains("locator digest"));
}

#[test]
fn external_output_without_exact_adapter_fails_closed() {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
    let operation_receipt = store
        .evidence_from_bytes(
            b"operation receipt",
            "operation-receipt",
            "application/json",
        )
        .unwrap();
    let verification_probe = store
        .evidence_from_bytes(b"stable probe", "verification-probe", "application/json")
        .unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: "external-task".to_string(),
        generation: 1,
        completion_contract: CompletionContract::Report,
        requirements_digest: ContentDigest::of_bytes(REQUIREMENTS),
        source_revision: "external@before".to_string(),
        outputs: vec![OutputRef::External(Box::new(ExternalOutput {
            adapter_kind: "calendar".to_string(),
            resource_id: "event-123".to_string(),
            before_digest: ContentDigest::of_bytes(b"before"),
            after_digest: ContentDigest::of_bytes(b"after"),
            operation_receipt,
            verification_probe,
        }))],
        validation_evidence: vec![evidence(&store)],
        worker_summary_digest: ContentDigest::of_bytes(SUMMARY),
    };

    let error = ReviewResolver::new(&store)
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(
        error.kind,
        IncompleteEvidenceKind::UnsupportedExternalAdapter
    );
}

#[test]
fn direct_mutable_path_locator_is_not_deserializable() {
    let value = serde_json::json!({
        "kind": "file",
        "path": "/tmp/mutable-report"
    });
    assert!(serde_json::from_value::<ImmutableLocator>(value).is_err());
}

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn git_diff(repository: &Path, base: &str, head: &str) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args([
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            base,
            head,
            "--",
        ])
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(output.status.success());
    output.stdout
}

fn git_fixture() -> (TempDir, String, String, String) {
    let temp = TempDir::new().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "user.name", "WG Test"]);
    git(temp.path(), &["config", "user.email", "wg@example.invalid"]);
    fs::write(temp.path().join("README.md"), "base\n").unwrap();
    git(temp.path(), &["add", "README.md"]);
    git(temp.path(), &["commit", "-q", "-m", "base"]);
    let base = git(temp.path(), &["rev-parse", "HEAD"]);
    fs::write(temp.path().join("README.md"), "base\nreviewed change\n").unwrap();
    git(temp.path(), &["add", "README.md"]);
    git(temp.path(), &["commit", "-q", "-m", "candidate"]);
    let head = git(temp.path(), &["rev-parse", "HEAD"]);
    let tree = git(temp.path(), &["rev-parse", "HEAD^{tree}"]);
    (temp, base, head, tree)
}

#[test]
fn land_resolves_exact_commit_tree_and_diff_without_worktree_reads() {
    let (repository, base, head, tree) = git_fixture();
    let store_temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(store_temp.path().join("store")).unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: "land-task".to_string(),
        generation: 3,
        completion_contract: CompletionContract::Land,
        requirements_digest: ContentDigest::of_bytes(REQUIREMENTS),
        source_revision: base.clone(),
        outputs: vec![OutputRef::Git(GitOutput {
            commit_oid: head.clone(),
            integrated_main_oid: base.clone(),
            tree_oid: tree.clone(),
            diff_bundle_digest: ContentDigest::of_bytes(&git_diff(repository.path(), &base, &head)),
        })],
        validation_evidence: vec![evidence(&store)],
        worker_summary_digest: ContentDigest::of_bytes(SUMMARY),
    };

    let bundle = ReviewResolver::new(&store)
        .repository(repository.path())
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap();

    let worksgood::completion_manifest::ResolvedOutput::Git {
        commit_oid,
        tree_oid,
        diff,
    } = &bundle.outputs[0]
    else {
        panic!("expected Git output")
    };
    assert_eq!(commit_oid, &head);
    assert_eq!(tree_oid, &tree);
    assert!(String::from_utf8_lossy(&diff.bytes).contains("reviewed change"));
}

#[test]
fn land_rejects_a_tree_containing_protected_control_plane_paths() {
    let (repository, base, _head, _tree) = git_fixture();
    fs::create_dir_all(repository.path().join(".wg")).unwrap();
    fs::write(repository.path().join(".wg/secret"), "must not land\n").unwrap();
    git(repository.path(), &["add", "-f", ".wg/secret"]);
    git(
        repository.path(),
        &["commit", "-q", "-m", "protected candidate"],
    );
    let head = git(repository.path(), &["rev-parse", "HEAD"]);
    let tree = git(repository.path(), &["rev-parse", "HEAD^{tree}"]);

    let store_temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(store_temp.path().join("store")).unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: "unsafe-land".to_string(),
        generation: 1,
        completion_contract: CompletionContract::Land,
        requirements_digest: ContentDigest::of_bytes(REQUIREMENTS),
        source_revision: base.clone(),
        outputs: vec![OutputRef::Git(GitOutput {
            commit_oid: head.clone(),
            integrated_main_oid: base.clone(),
            tree_oid: tree,
            diff_bundle_digest: ContentDigest::of_bytes(&git_diff(repository.path(), &base, &head)),
        })],
        validation_evidence: vec![evidence(&store)],
        worker_summary_digest: ContentDigest::of_bytes(SUMMARY),
    };

    let error = ReviewResolver::new(&store)
        .repository(repository.path())
        .resolve(&manifest, REQUIREMENTS, SUMMARY)
        .unwrap_err();
    assert_eq!(error.kind, IncompleteEvidenceKind::ProtectedControlPlane);
}
