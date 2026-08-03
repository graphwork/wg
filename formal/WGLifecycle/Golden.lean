import WGLifecycle.Incident
import WGLifecycle.DaemonPlanner

namespace WGLifecycle.Golden

open WGLifecycle
open WGLifecycle.Incident
open WGLifecycle.DaemonPlanner

/-- The committed JSON golden names and this module are a versioned pair. -/
def happy (d : Disposition) : State :=
  run s0 [
    .candidateValidated cap candidate,
    .childSettled cap candidate 100,
    .wrapperHandoff cap d 101,
    .promote cap candidate.id candidate.baseCas candidate.baseCas,
    .commitCleanup cap]

theorem happy_land : (happy .land).phase = .done ∧ (happy .land).promotionCount = 1 := by
  native_decide

theorem happy_deliver : (happy .deliver).phase = .done ∧
    ∃ tx, (happy .deliver).finishTx = some tx ∧ tx.disposition = .deliver := by
  native_decide

theorem happy_report : (happy .report).phase = .done ∧
    ∃ tx, (happy .report).finishTx = some tx ∧ tx.disposition = .report := by
  native_decide

theorem stale_unrelated_caller :
    (reduce s0 (.wrapperHandoff stale .land 101)) =
      (s0, .rejected .staleCapability) := by
  native_decide

def deadWithoutReceipt : State :=
  (reduce s0 (.ownerProvenDead cap true 101)).1

def continued : State :=
  (reduce deadWithoutReceipt (.resumeSame cap 2 2)).1

theorem owner_death_same_session_continuation :
    continued.phase = .running ∧ continued.pending = none ∧
    ∃ next, continued.owner = some next ∧ continued.worktreeLease = some next ∧
      continued.sessionLease = some next ∧ next.taskId = cap.taskId ∧
      next.attemptId = cap.attemptId ∧ next.generation = cap.generation ∧
      next.fence = cap.fence ∧ next.wrapperEpoch = 2 ∧ next.childEpoch = 2 ∧
      next.wrapperIdentityDigest = cap.wrapperIdentityDigest ∧
      next.childIdentityDigest = cap.childIdentityDigest ∧ next.ownedChild = true := by
  native_decide

theorem lost_finish_response :
    (reduce s5 (.commitCleanup cap)).1 = s5 ∧
    (reduce s5 (.commitCleanup cap)).2 = .noop := by
  native_decide

def txCut : State :=
  (reduce s2 (.beginFinish cap .land)).1

theorem cas_target_movement :
    (reduce txCut (.promote cap candidate.id candidate.baseCas 999)).1 = txCut ∧
    (reduce txCut (.promote cap candidate.id candidate.baseCas 999)).2 =
      .rejected .casMoved := by
  native_decide

theorem crash_before_tx :
    (reduce s2 .crash).1 = s2 ∧ recoveryRank s2 = 3 := by
  native_decide

theorem crash_after_tx :
    (reduce txCut .crash).1 = txCut ∧ recoveryRank txCut = 2 := by
  native_decide

def promotedCut : State :=
  (reduce txCut (.promote cap candidate.id candidate.baseCas candidate.baseCas)).1

theorem crash_after_promotion :
    (reduce promotedCut .crash).1 = promotedCut ∧ recoveryRank promotedCut = 1 := by
  native_decide

theorem crash_after_cleanup :
    (reduce s5 .crash).1 = s5 ∧ recoveryRank s5 = 0 := by
  native_decide

theorem double_promotion_replay :
    (reduce promotedCut (.promote cap candidate.id candidate.baseCas candidate.baseCas)).1 =
      promotedCut ∧
    (reduce promotedCut (.promote cap candidate.id candidate.baseCas candidate.baseCas)).2 =
      .noop ∧ promotedCut.promotionCount = 1 := by
  native_decide

theorem message_cannot_resurrect :
    (reduce s5 (.message 42)).1 = s5 ∧ (reduce s5 (.candidateValidated cap candidate)).1 = s5 := by
  native_decide

theorem contention_breaker_neutral :
    (reduce s0 (.ownershipContention stale)).1.breakerCharges = s0.breakerCharges := by
  native_decide

def unsafeCandidate : Candidate := { candidate with protectedFree := false }

theorem protected_candidate_mutation_rejected :
    (reduce s0 (.candidateValidated cap unsafeCandidate)).1 = s0 ∧
    (reduce s0 (.candidateValidated cap unsafeCandidate)).2 =
      .rejected .candidateNotProtected := by
  native_decide

def deadTxCut : State := (reduce txCut (.ownerProvenDead cap true 101)).1

def deadPromotedCut : State := (reduce promotedCut (.ownerProvenDead cap true 101)).1

def observedCleaned : State := { s5 with ownerProvenDead := true }

/-- Lean mirror of `formal/fixtures/daemon/v2`: exact incident classes,
explicit evidence presence, and the finite one-shot budget select the same
normalized disposition names as Rust replay. -/
theorem failed_prerequisite_v2_fixtures :
    plannerSchemaVersion = 2 ∧ traceSchemaVersion = 2 ∧
    decideFailedPrerequisite .providerUnavailableAfterDurableCandidate
      ⟨true, true, true, true⟩ 0 1 = .replanFinish ∧
    decideFailedPrerequisite .sourceExecutionNoProgress
      ⟨false, false, true, true⟩ 0 1 = .retryFailedPrerequisite ∧
    decideFailedPrerequisite .orphanBeforeSpawn
      ⟨false, false, false, false⟩ 0 1 = .retryFailedPrerequisite ∧
    decideFailedPrerequisite .semanticValidationRejected
      ⟨true, true, true, true⟩ 0 1 = .semanticRepairWait := by
  native_decide

/-- Exact runtime reducer wire semantics for every committed JSON crash cut. -/
theorem exited_worker_runtime_wire_incident :
    exitedWorkerFinishReducerVersion = 1 ∧
    finishConvergenceRank deadWithoutReceipt = .awaitReceipt ∧
    finishConvergenceAction cap cap deadWithoutReceipt = .resumeSameSession ∧
    finishConvergenceRank deadCut = .receiptNoTransaction ∧
    finishConvergenceAction cap cap deadCut = .resumeSameSession ∧
    finishConvergenceRank deadTxCut = .transactionDurable ∧
    finishConvergenceAction cap cap deadTxCut = .promote ∧
    finishConvergenceRank deadPromotedCut = .promoted ∧
    finishConvergenceAction cap cap deadPromotedCut = .cleanup ∧
    finishConvergenceRank observedCleaned = .cleaned ∧
    finishConvergenceAction cap cap observedCleaned = .complete ∧
    finishConvergenceAction stale cap deadCut = .rejectStale := by
  native_decide

end WGLifecycle.Golden

namespace WGLifecycle.V2.Golden

open WGLifecycle.V2

def binding : Binding := {
  generation := 2, attempt := 1, fence := 7, worktreeLease := 3,
  candidate := 1001, base := 500 }

def happyEvents : List Event := [
  .advance binding .prepared true, .advance binding .quiescing true,
  .advance binding .workSaved true, .advance binding .candidateSealed true,
  .advance binding .validated true, .advance binding .accepted true,
  .advance binding .dispositionRecorded true, .advance binding .effectPrepared true,
  .advance binding .effectCommitted true, .advance binding .cleanupPrepared true,
  .advance binding .cleanupCommitted true, .graphSave binding true]

def happy : State := run (initial binding) happyEvents

theorem happy_land_deliver_report_v2 :
    happy.phase = .graphSaved ∧ happy.graphSaveValid = true ∧
    happy.dependencySatisfied = true ∧ happy.workSaved = true ∧
    happy.accepted = true ∧ happy.effectCount = 1 ∧ happy.cleanupCommitted = true := by
  native_decide

theorem missing_each_receipt_blocks_done :
    (reduce (initial binding) (.graphSave binding true)).2 = .rejected ∧
    (reduce { initial binding with phase := .cleanupCommitted }
      (.graphSave binding false)).2 = .rejected := by
  native_decide

theorem stale_actor_v2 :
    (reduce (initial binding)
      (.advance { binding with fence := 8 } .prepared true)).2 = .rejected := by
  native_decide

theorem duplicate_effect_v2 :
    let prepared := { initial binding with
      phase := .effectPrepared
      workSaved := true
      accepted := true }
    let committed := (reduce prepared (.advance binding .effectCommitted true)).1
    (reduce committed (.advance binding .effectCommitted true)).1 = committed ∧
      committed.effectCount = 1 := by
  native_decide

end WGLifecycle.V2.Golden
