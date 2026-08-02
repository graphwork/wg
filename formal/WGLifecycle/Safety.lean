import WGLifecycle.Model
import Std.Tactic

namespace WGLifecycle

/-- Every authority-bearing event exposes its process capability. -/
def Event.caller? : Event → Option Capability
  | .candidateValidated c _ | .childSettled c _ _ | .ownerProvenDead c _ _
  | .wrapperHandoff c _ _ | .resumeSame c _ _ | .beginFinish c _
  | .promote c _ _ _ | .commitCleanup c | .fail c | .ownershipContention c => some c
  | .message _ | .crash => none

/-- Stale attempt/generation/fence/process capabilities cannot change state. -/
theorem attempt_fencing
    (hcaller : event.caller? = some caller)
    (hstale : exactOwner s caller = false) :
    (reduce s event).1 = s := by
  cases event <;> simp [Event.caller?] at hcaller
  all_goals
    subst_vars
    by_cases hv : s.version = wireVersion <;>
      by_cases hp : s.phase = .running <;>
      simp [reduce, reject, exactFinishOwner, hstale, hv, hp]

/-- There is at most one writer in each attempt/worktree/session/finish lease slot. -/
def SingleOwnership (s : State) : Prop :=
  (∀ a b, s.owner = some a → s.owner = some b → a = b) ∧
  (∀ a b, s.worktreeLease = some a → s.worktreeLease = some b → a = b) ∧
  (∀ a b, s.sessionLease = some a → s.sessionLease = some b → a = b) ∧
  (∀ a b, s.finishLease = some a → s.finishLease = some b → a = b)

theorem single_ownership (s : State) : SingleOwnership s := by
  constructor
  · intro a b ha hb; rw [ha] at hb; exact Option.some.inj hb
  constructor
  · intro a b ha hb; rw [ha] at hb; exact Option.some.inj hb
  constructor
  · intro a b ha hb; rw [ha] at hb; exact Option.some.inj hb
  · intro a b ha hb; rw [ha] at hb; exact Option.some.inj hb

/-- Composite safety predicate preserved by every executable transition. -/
def ProtocolInvariant (s : State) : Prop :=
  s.version = wireVersion ∧
  s.worktreeLease = s.owner ∧
  s.sessionLease = s.owner ∧
  (∀ c, s.finishLease = some c → s.owner = some c) ∧
  (s.phase = .running → s.finishTx.isSome = s.finishLease.isSome) ∧
  s.promotionCount ≤ 1 ∧
  (∀ c, s.accepted = some c → c.protectedFree = true) ∧
  (∀ tx, s.finishTx = some tx → s.accepted = some tx.candidate) ∧
  (s.promotionCount = 1 → ∃ tx, s.finishTx = some tx ∧
    tx.promotionReceipt = true ∧ s.accepted = some tx.candidate) ∧
  (∀ tx, s.finishTx = some tx → tx.promotionReceipt = true →
    s.promotionCount = 1) ∧
  (s.phase = .done → ∃ tx, s.finishTx = some tx ∧
    tx.promotionReceipt = true ∧ tx.cleanupCommitted = true ∧
    s.promotionCount = 1 ∧ s.owner = none ∧
    s.worktreeLease = none ∧ s.sessionLease = none ∧ s.finishLease = none)

private theorem initial_invariant (cap : Capability) :
    ProtocolInvariant (initial cap) := by
  simp [ProtocolInvariant, initial]

set_option maxHeartbeats 2000000 in
theorem reduce_preserves_invariant
    (h : ProtocolInvariant s) : ProtocolInvariant (reduce s event).1 := by
  cases event <;>
    simp [reduce, reject, exactOwner, exactFinishOwner, ProtocolInvariant] at *
  all_goals repeat' split
  all_goals simp_all
  all_goals grind

/-- All named composite invariants hold over every reachable state. -/
theorem reachable_protocol_invariant
    (hr : Reachable cap s) : ProtocolInvariant s := by
  induction hr with
  | init => exact initial_invariant cap
  | step hr hstep ih =>
    rw [← hstep]
    exact reduce_preserves_invariant ih

/-- Worktree/session/attempt ownership agrees; a finish lease has that same owner. -/
theorem reachable_single_ownership
    (hr : Reachable cap s) :
    SingleOwnership s ∧ s.worktreeLease = s.owner ∧ s.sessionLease = s.owner ∧
      (∀ c, s.finishLease = some c → s.owner = some c) := by
  rcases reachable_protocol_invariant hr with
    ⟨_, hworktree, hsession, hfinish, _⟩
  exact ⟨single_ownership s, hworktree, hsession, hfinish⟩

/-- Once terminal, every message and late write is state-inert. -/
theorem terminal_cannot_resurrect
    (hterminal : s.phase ≠ .running) : (reduce s event).1 = s := by
  by_cases hv : s.version = wireVersion <;>
    simp [reduce, reject, hterminal, hv]

/-- Message payloads have no lifecycle authority. -/
theorem inert_messages
    (hversion : s.version = wireVersion) :
    (reduce s (.message body)).1.phase = s.phase ∧
    (reduce s (.message body)).1.owner = s.owner ∧
    (reduce s (.message body)).1.finishTx = s.finishTx := by
  simp [reduce, hversion]
  split <;> simp_all

/-- The first terminal result wins because all later transitions are inert. -/
theorem first_terminal_result_wins
    (_hr : Reachable cap s) (hterminal : s.phase = .done ∨ s.phase = .failed) :
    (reduce s event).1 = s := by
  apply terminal_cannot_resurrect
  rcases hterminal with h | h <;> simp [h]

/-- Finish/promotion is at most once and only for the accepted candidate/base CAS. -/
theorem finish_promotion_at_most_once
    (hr : Reachable cap s) :
    s.promotionCount ≤ 1 ∧
    (∀ tx, s.finishTx = some tx → s.accepted = some tx.candidate) ∧
    (s.promotionCount = 1 → ∃ tx, s.finishTx = some tx ∧
      tx.promotionReceipt = true ∧ s.accepted = some tx.candidate) := by
  rcases reachable_protocol_invariant hr with
    ⟨_, _, _, _, _, hcount, _, hexact, hpromoted, _⟩
  exact ⟨hcount, hexact, hpromoted⟩

/-- Done implies durable Land/Deliver/Report disposition and cleanup committed. -/
theorem done_implies_disposition_and_cleanup
    (hr : Reachable cap s) (hdone : s.phase = .done) :
    ∃ tx, s.finishTx = some tx ∧ tx.promotionReceipt = true ∧
      tx.cleanupCommitted = true ∧ s.promotionCount = 1 ∧
      s.owner = none ∧ s.worktreeLease = none ∧ s.sessionLease = none ∧
      s.finishLease = none := by
  rcases reachable_protocol_invariant hr with
    ⟨_, _, _, _, _, _, _, _, _, _, hdoneSafe⟩
  exact hdoneSafe hdone

/-- Ordinary dependencies are satisfied only by successful dispositions. -/
theorem dependency_only_successful
    (_hr : Reachable cap s) (hdep : dependencySatisfied s) :
    ∃ tx, s.phase = .done ∧ s.finishTx = some tx ∧
      tx.promotionReceipt = true ∧ tx.cleanupCommitted = true := by
  rcases hdep with ⟨hdone, tx, htx, hpromoted, hclean⟩
  exact ⟨tx, hdone, htx, hpromoted, hclean⟩

/-- `.wg` control-plane identity is never part of a reachable candidate projection. -/
theorem protected_resource_invariant
    (hr : Reachable cap s) :
    (∀ c, s.accepted = some c → c.protectedFree = true) ∧
    (∀ tx, s.finishTx = some tx → tx.candidate.protectedFree = true) := by
  rcases reachable_protocol_invariant hr with
    ⟨_, _, _, _, _, _, hprotected, hexact, _⟩
  constructor
  · exact hprotected
  · intro tx htx
    exact hprotected tx.candidate (hexact tx htx)

/-- The exact current wrapper owns the child topology and can create the handoff tx. -/
theorem exact_wrapper_handoff_authorized
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (halive : s.ownerProvenDead = false)
    (hwrapper : caller.wrapperEpoch ≠ 0)
    (hchild : caller.childEpoch ≠ 0)
    (hwrapperId : caller.wrapperIdentityDigest ≠ 0)
    (hchildId : caller.childIdentityDigest ≠ 0)
    (howned : caller.ownedChild = true)
    (haccepted : s.accepted = some candidate)
    (htx : s.finishTx = none) :
    (reduce s (.wrapperHandoff caller disposition deadline)).1.finishTx =
      some {
        candidate := candidate
        disposition := disposition
        promotionReceipt := false
        cleanupCommitted := false } ∧
    (reduce s (.wrapperHandoff caller disposition deadline)).1.finishLease = some caller := by
  simp [reduce, hversion, hrunning, howner, halive, hwrapper, hchild, hwrapperId,
    hchildId, howned, haccepted, htx]

/-- An unrelated wrapper never gains authority merely by process ancestry. -/
theorem unrelated_wrapper_rejected
    (hstale : exactOwner s caller = false) :
    (reduce s (.wrapperHandoff caller disposition deadline)).1 = s := by
  apply attempt_fencing (caller := caller)
  · simp [Event.caller?]
  · exact hstale

end WGLifecycle
