import WGLifecycle.Convergence
import Std.Tactic

namespace WGLifecycle.DaemonPlanner

/-- Wire versions shared with `service::planner`. -/
def plannerSchemaVersion : Nat := 1
def traceSchemaVersion : Nat := 1

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

/-- Conditional liveness names useful scheduler fairness rather than OS truth. -/
structure PlannerEnvironment (enabled : CorrectedDisposition → Prop) : Prop where
  fairUsefulScheduling : ∀ disposition, enabled disposition

/-- Under explicit fair scheduling, every corrected incident has an enabled action/wait. -/
theorem corrected_incident_conditionally_live
    (env : PlannerEnvironment enabled) (incident : Incident) :
    enabled (correctedDisposition incident) := by
  exact env.fairUsefulScheduling (correctedDisposition incident)

end WGLifecycle.DaemonPlanner
