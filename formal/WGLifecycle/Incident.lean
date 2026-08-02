import WGLifecycle.Convergence
import Std.Tactic

namespace WGLifecycle.Incident

open WGLifecycle

/-- `fix-candidate-wg-control-plane-destruction`, generation 0, attempt-0-1. -/
def cap : Capability := {
  taskId := 949
  generation := 0
  attemptId := 1
  attemptFence := 1
  wrapperEpoch := 1
  childEpoch := 1
  wrapperIdentityDigest := 10001
  childIdentityDigest := 10002
  ownedChild := true
}

def stale : Capability := {
  taskId := 949
  generation := 0
  attemptId := 0
  attemptFence := 0
  wrapperEpoch := 9
  childEpoch := 9
  wrapperIdentityDigest := 90001
  childIdentityDigest := 90002
  ownedChild := true
}

def candidate : Candidate := {
  id := 1001
  baseCas := 500
  protectedFree := true
}

def s0 : State := initial cap

def s1 : State := (reduce s0 (.candidateValidated cap candidate)).1

/-- Native child settles/exits; the wrapper remains the exact live owner. -/
def s2 : State := (reduce s1 (.childSettled cap candidate 100)).1

/-- Corrected topology accepts the owning wrapper after its child exit observation. -/
def s3 : State := (reduce s2 (.wrapperHandoff cap .land 101)).1

def s4 : State := (reduce s3 (.promote cap candidate.id candidate.baseCas candidate.baseCas)).1

def s5 : State := (reduce s4 (.commitCleanup cap)).1

/-- Alternate crash cut: the wrapper itself dies before creating the transaction. -/
def deadCut : State := (reduce s2 (.ownerProvenDead cap true 101)).1

/-- Runtime may resume the exact attempt/session/worktree rather than infer success. -/
def continued : State := (reduce deadCut (.resumeSame cap 2 2)).1

/-- The production crash cut really has no finish tx before wrapper handoff. -/
theorem exact_crash_cut_has_no_tx : s2.finishTx = none := by native_decide

/-- Child settlement is never parking: the live wrapper has action and deadline. -/
theorem exact_crash_cut_has_action :
    s2.pending = some .beginFinish ∧ s2.deadline = some 100 := by native_decide

/-- Exact wrapper/native-child capability creates the missing transaction. -/
theorem exact_wrapper_handoff_creates_tx :
    s3.finishTx.isSome = true ∧ s3.pending = some .promote := by native_decide

/-- The incident converges without generation/session/worktree replacement. -/
theorem same_lease_until_cleanup :
    s3.owner = some cap ∧ s3.worktreeLease = some cap ∧ s3.sessionLease = some cap ∧
      s3.finishLease = some cap := by
  native_decide

/-- Final state has one promotion, a durable receipt, and committed cleanup. -/
theorem incident_converges_exactly_once :
    s5.phase = .done ∧ s5.promotionCount = 1 ∧ s5.finishLease = none ∧
    ∃ tx, s5.finishTx = some tx ∧ tx.promotionReceipt = true ∧
      tx.cleanupCommitted = true := by
  native_decide

/-- If the wrapper dies first, it cannot act; the same-session action is explicit. -/
theorem dead_wrapper_schedules_same_session :
    deadCut.finishTx = none ∧ deadCut.pending = some .resumeSame ∧
      deadCut.deadline = some 101 ∧
      (reduce deadCut (.wrapperHandoff cap .land 102)).1 = deadCut := by
  native_decide

/-- Same-session recovery retains the immutable attempt/fence and both leases. -/
theorem dead_wrapper_continues_without_competitor :
    ∃ next, continued.owner = some next ∧ continued.worktreeLease = some next ∧
      continued.sessionLease = some next ∧ next.taskId = cap.taskId ∧
      next.generation = cap.generation ∧ next.attemptId = cap.attemptId ∧
      next.attemptFence = cap.attemptFence ∧ continued.pending = none := by
  native_decide

/-- An unrelated stale process remains inert at the exact crash cut. -/
theorem stale_wrapper_cannot_finish :
    (reduce s2 (.wrapperHandoff stale .land 102)).1 = s2 := by native_decide

/-- Late messages cannot resurrect the corrected terminal result. -/
theorem incident_late_message_inert :
    (reduce s5 (.message 42)).1 = s5 := by native_decide

/-- Therefore neither corrected branch reaches the old unscheduled parking state. -/
theorem motivating_trace_not_stuck :
    (s2.pending.isSome = true ∧ s2.deadline.isSome = true) ∧
    (deadCut.pending = some .resumeSame ∧ deadCut.deadline.isSome = true) ∧
    (continued.phase = .running ∧ continued.pending = none) ∧
    (s5.phase = .done ∧ s5.pending = none) := by
  native_decide

end WGLifecycle.Incident
