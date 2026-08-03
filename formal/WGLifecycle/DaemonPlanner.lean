import WGLifecycle.Convergence
import Std.Tactic

namespace WGLifecycle.DaemonPlanner

/-- Wire versions shared with `service::planner`. -/
def plannerSchemaVersion : Nat := 3
def traceSchemaVersion : Nat := 3

/-- The four and only four admissible forward explanations for unfinished work. -/
structure ForwardProjection where
  runnable : Bool
  authenticatedLiveOwner : Bool
  externalWait : Bool
  scheduledConvergence : Bool
  deriving DecidableEq, Repr

def forwardCount (p : ForwardProjection) : Nat :=
  (if p.runnable then 1 else 0) +
  (if p.authenticatedLiveOwner then 1 else 0) +
  (if p.externalWait then 1 else 0) +
  (if p.scheduledConvergence then 1 else 0)

def ForwardExhaustive (p : ForwardProjection) : Prop := forwardCount p = 1

inductive Incident where
  | exitedWrapperRejectedStale
  | reopenBeforeOwnerRelease
  | parkResumeOverlap
  | obsoleteDaemonChatCreationLostResponse
  | targetMovedDuringFinish
  | surpriseArchivalBacklog
  | controlPlaneCandidateReplacement
  | deadPiOwnerRetainingLeases
  | abandonedDependencySatisfiedReadiness
  deriving DecidableEq, Repr

inductive CorrectedDisposition where
  | runnable | authenticatedLiveOwner | externalWait | scheduledConvergence
  deriving DecidableEq, Repr

/-- Historical rules expose the incident instead of silently parking it. -/
def historicalViolation (incident : Incident) : Incident := incident

/-- Corrected rules choose a single safe class, never an untyped ambient action. -/
def correctedDisposition : Incident → CorrectedDisposition
  | .parkResumeOverlap | .surpriseArchivalBacklog
  | .abandonedDependencySatisfiedReadiness => .externalWait
  | .exitedWrapperRejectedStale | .reopenBeforeOwnerRelease
  | .obsoleteDaemonChatCreationLostResponse | .targetMovedDuringFinish
  | .controlPlaneCandidateReplacement | .deadPiOwnerRetainingLeases =>
      .scheduledConvergence

def projectionOf : CorrectedDisposition → ForwardProjection
  | .runnable => ⟨true, false, false, false⟩
  | .authenticatedLiveOwner => ⟨false, true, false, false⟩
  | .externalWait => ⟨false, false, true, false⟩
  | .scheduledConvergence => ⟨false, false, false, true⟩

/-- Every seeded historical failure has exactly one corrected forward explanation. -/
theorem every_incident_corrected_exhaustive (incident : Incident) :
    ForwardExhaustive (projectionOf (correctedDisposition incident)) := by
  cases incident <;>
    simp [ForwardExhaustive, projectionOf, correctedDisposition, forwardCount]

inductive FailedPrerequisiteClass where
  | providerUnavailableAfterDurableCandidate
  | sourceExecutionNoProgress
  | sourceExecutionWithProgress
  | orphanBeforeSpawn
  | semanticValidationRejected
  deriving DecidableEq, Repr

structure FailedPrerequisiteEvidence where
  workSave : Bool
  candidate : Bool
  session : Bool
  worktree : Bool
  deriving DecidableEq, Repr

inductive FailedPrerequisiteDecision where
  | replanFinish
  | retryFailedPrerequisite
  | recordNeedsReconciliation
  | semanticRepairWait
  deriving DecidableEq, Repr

/-- Pure counterpart of the Rust v2 failed-prerequisite projection. A finite
retry budget is an input fact; malformed provider evidence fails closed. -/
def decideFailedPrerequisite (failureClass : FailedPrerequisiteClass)
    (evidence : FailedPrerequisiteEvidence) (automaticRetries maxAutomaticRetries : Nat) :
    FailedPrerequisiteDecision :=
  match failureClass with
  | .semanticValidationRejected => .semanticRepairWait
  | .providerUnavailableAfterDurableCandidate =>
      if automaticRetries < maxAutomaticRetries ∧ evidence.workSave ∧ evidence.candidate then
        .replanFinish
      else
        .recordNeedsReconciliation
  | .sourceExecutionNoProgress | .sourceExecutionWithProgress | .orphanBeforeSpawn =>
      if automaticRetries < maxAutomaticRetries then
        .retryFailedPrerequisite
      else
        .recordNeedsReconciliation

def failedPrerequisiteProjection : FailedPrerequisiteDecision → ForwardProjection
  | .semanticRepairWait => projectionOf .externalWait
  | .replanFinish | .retryFailedPrerequisite | .recordNeedsReconciliation =>
      projectionOf .scheduledConvergence

/-- Descendant exhaustiveness under typed failure classification assumptions:
every unfinished descendant blocked on one classified failed prerequisite has
exactly one owner/action/wait/deadline class. -/
theorem failed_prerequisite_descendant_exhaustive
    (failureClass : FailedPrerequisiteClass) (evidence : FailedPrerequisiteEvidence)
    (automaticRetries maxAutomaticRetries : Nat) :
    ForwardExhaustive
      (failedPrerequisiteProjection
        (decideFailedPrerequisite failureClass evidence automaticRetries maxAutomaticRetries)) := by
  cases failureClass <;>
    simp [decideFailedPrerequisite, failedPrerequisiteProjection, ForwardExhaustive,
      projectionOf, forwardCount]
  all_goals split <;> simp_all [failedPrerequisiteProjection, projectionOf, forwardCount]

/-- Semantic rejection is terminal and can only project an explicit repair
wait; it never selects either automatic retry action. -/
theorem semantic_rejection_never_retries
    (evidence : FailedPrerequisiteEvidence) (automaticRetries maxAutomaticRetries : Nat) :
    decideFailedPrerequisite .semanticValidationRejected evidence automaticRetries
      maxAutomaticRetries = .semanticRepairWait := by
  rfl

/-- Fail-closed repair for arbitrary malformed projections. -/
def normalizeForward (unfinished : Bool) (p : ForwardProjection) : ForwardProjection :=
  if !unfinished then ⟨false, false, false, false⟩
  else if forwardCount p = 1 then p
  else projectionOf .scheduledConvergence

theorem unfinished_normalization_exhaustive
    (h : unfinished = true) : ForwardExhaustive (normalizeForward unfinished p) := by
  simp [normalizeForward, h, ForwardExhaustive]
  split <;> simp_all [projectionOf, forwardCount]

inductive EffectStatus where
  | issued | acknowledged
  deriving DecidableEq, Repr

structure LogicalEffect where
  id : Nat
  status : EffectStatus
  deriving DecidableEq, Repr

/-- A duplicate/reordered acknowledgement changes status, never effect identity. -/
def acknowledge (effect : LogicalEffect) : LogicalEffect :=
  { effect with status := .acknowledged }

theorem duplicate_ack_idempotent :
    acknowledge (acknowledge effect) = acknowledge effect := by
  cases effect <;> rfl

theorem acknowledgement_preserves_identity :
    (acknowledge effect).id = effect.id := by
  rfl

inductive WorktreeOwnerProof where
  | authenticatedLive
  | provenDead
  | unproven
  deriving DecidableEq, Repr

inductive WorktreeSpawnAction where
  | reclaimRetainDeadOwner
  | dispatchCurrentAttempt
  deriving DecidableEq, Repr

/-- Pure v3 preparation plan. Only proven death plus both retained evidence
receipts authorizes the two-effect sequence. Live ownership remains protected;
missing proof/evidence emits no mutation. -/
def planWorktreeSpawn (owner : WorktreeOwnerProof)
    (ownerToken observerState : Bool) : List WorktreeSpawnAction :=
  match owner with
  | .provenDead =>
      if ownerToken ∧ observerState then
        [.reclaimRetainDeadOwner, .dispatchCurrentAttempt]
      else
        []
  | .authenticatedLive | .unproven => []

theorem proven_dead_worktree_reclaimed_then_dispatched_once :
    planWorktreeSpawn .provenDead true true =
      [.reclaimRetainDeadOwner, .dispatchCurrentAttempt] := by
  rfl

theorem proven_live_worktree_is_protected (ownerToken observerState : Bool) :
    planWorktreeSpawn .authenticatedLive ownerToken observerState = [] := by
  rfl

theorem worktree_reclaim_replay_is_deterministic
    (owner : WorktreeOwnerProof) (ownerToken observerState : Bool) :
    planWorktreeSpawn owner ownerToken observerState =
      planWorktreeSpawn owner ownerToken observerState := by
  rfl

/-- Conditional liveness names useful scheduler fairness rather than OS truth. -/
structure PlannerEnvironment (enabled : CorrectedDisposition → Prop) : Prop where
  fairUsefulScheduling : ∀ disposition, enabled disposition

/-- Under explicit fair scheduling, every corrected incident has an enabled action/wait. -/
theorem corrected_incident_conditionally_live
    (env : PlannerEnvironment enabled) (incident : Incident) :
    enabled (correctedDisposition incident) := by
  exact env.fairUsefulScheduling (correctedDisposition incident)

end WGLifecycle.DaemonPlanner
