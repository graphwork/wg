import WGLifecycle.Incident

namespace WGLifecycle.Golden

open WGLifecycle
open WGLifecycle.Incident

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
