import WGLifecycle.Safety

namespace WGLifecycle

/-- Abstract durable-store predicate supplied by the implementation boundary. -/
abbrev DurableStore := State → Prop

/--
A scheduler action is useful only when it executes the pending same-session
continuation or strictly decreases the finish recovery rank. Merely delivering
an inert message does not satisfy fairness.
-/
def DeterministicRecoveryAction (s : State) (event : Event) : Prop :=
  ((s.pending = some .resumeSame) ∧ (reduce s event).2 = .applied ∧
    (reduce s event).1.pending = none ∧
    (reduce s event).1.owner.isSome = true) ∨
  ((s.pending ≠ some .resumeSame) ∧ (reduce s event).2 = .applied ∧
    recoveryRank (reduce s event).1 < recoveryRank s)

/--
Environmental facts are theorem parameters, not hidden correctness claims.
WG must supply durable persistence, eventual restart, useful fair scheduling,
and truthful proven-dead observations at the Rust/OS boundary.
-/
structure EnvironmentAssumptions (cap : Capability) (durable : DurableStore) : Prop where
  durableInitial : durable (initial cap)
  durableStep : ∀ {s s' event}, durable s → Step s event s' → durable s'
  eventualRestart : ∀ {s}, Reachable cap s → s.pending.isSome = true → durable s
  fairScheduling : ∀ {s}, Reachable cap s → durable s → s.pending.isSome = true →
    ∃ event, DeterministicRecoveryAction s event
  truthfulProvenDead : ∀ {s caller deadline},
    (reduce s (.ownerProvenDead caller true deadline)).2 = .applied →
    s.owner = some caller

/-- A settled accepted attempt cannot be represented without an action/deadline. -/
theorem needs_finalization_not_parking
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hprotected : candidate.protectedFree = true)
    (haccepted : s.accepted = some candidate) :
    let next := (reduce s (.childSettled caller candidate deadline)).1
    next.pending = some .beginFinish ∧ next.deadline = some deadline := by
  simp [reduce, hversion, hrunning, howner, hprotected, haccepted]

/-- A truthful owner-death observation always schedules finish or same-session resume. -/
theorem proven_dead_schedules_action
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true) :
    let next := (reduce s (.ownerProvenDead caller true deadline)).1
    next.pending.isSome = true ∧ next.deadline = some deadline := by
  simp [reduce, hversion, hrunning, howner]

/-- The receipt/no-tx crash cut replays to a tx and strictly lowers rank 3 -> 2. -/
theorem replay_before_tx_rank_decreases
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hsettled : s.settled = true)
    (hpending : s.pending = some .beginFinish)
    (haccepted : s.accepted = some candidate)
    (htx : s.finishTx = none) :
    recoveryRank (reduce s (.beginFinish caller disposition)).1 < recoveryRank s := by
  simp [reduce, recoveryRank, hversion, hrunning, howner, hsettled,
    hpending, haccepted, htx]

/-- The tx/no-promotion crash cut replays exact candidate/base CAS, rank 2 -> 1. -/
theorem replay_after_tx_rank_decreases
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hfinishOwner : s.finishLease = some caller)
    (htx : s.finishTx = some tx)
    (hunpromoted : tx.promotionReceipt = false)
    (hclean : tx.cleanupCommitted = false) :
    recoveryRank
      (reduce s (.promote caller tx.candidate.id tx.candidate.baseCas
        tx.candidate.baseCas)).1 < recoveryRank s := by
  simp [reduce, recoveryRank, exactFinishOwner, hversion, hrunning, howner,
    hfinishOwner, htx, hunpromoted, hclean]

/-- The promoted/no-cleanup crash cut replays cleanup, rank 1 -> 0. -/
theorem replay_after_promotion_rank_decreases
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hfinishOwner : s.finishLease = some caller)
    (htx : s.finishTx = some tx)
    (hpromoted : tx.promotionReceipt = true)
    (hclean : tx.cleanupCommitted = false) :
    recoveryRank (reduce s (.commitCleanup caller)).1 < recoveryRank s := by
  simp [reduce, recoveryRank, exactFinishOwner, hversion, hrunning, howner,
    hfinishOwner, htx, hpromoted, hclean]

/-- Every non-cleaned finish crash cut has a replayable rank-decreasing action. -/
theorem every_finish_crash_cut_replayable
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hsettled : s.settled = true)
    (haccepted : s.accepted = some candidate)
    (hcut :
      (s.finishTx = none ∧ s.pending = some .beginFinish) ∨
      (∃ tx, s.finishTx = some tx ∧ s.finishLease = some caller ∧
        tx.cleanupCommitted = false)) :
    ∃ event, recoveryRank (reduce s event).1 < recoveryRank s := by
  rcases hcut with ⟨hnone, hpending⟩ | ⟨tx, htx, hfinishOwner, hclean⟩
  · refine ⟨.beginFinish caller .land, ?_⟩
    exact replay_before_tx_rank_decreases hversion hrunning howner hsettled
      hpending haccepted hnone
  · cases hp : tx.promotionReceipt
    · refine ⟨.promote caller tx.candidate.id tx.candidate.baseCas tx.candidate.baseCas, ?_⟩
      exact replay_after_tx_rank_decreases hversion hrunning howner hfinishOwner htx hp hclean
    · refine ⟨.commitCleanup caller, ?_⟩
      exact replay_after_promotion_rank_decreases hversion hrunning howner hfinishOwner htx hp hclean

/-- The deterministic finish replay plan used after settlement/restart. -/
def finishPlan (caller : Capability) (candidate : Candidate)
    (disposition : Disposition) : List Event :=
  [.beginFinish caller disposition,
   .promote caller candidate.id candidate.baseCas candidate.baseCas,
   .commitCleanup caller]

/--
The complete deterministic finish plan reaches exactly one durable successful
receipt and cleanup from the receipt/no-transaction crash cut.
-/
theorem deterministic_finish_plan_converges
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hsettled : s.settled = true)
    (hpending : s.pending = some .beginFinish)
    (haccepted : s.accepted = some candidate)
    (htx : s.finishTx = none) :
    let final := run s (finishPlan caller candidate disposition)
    final.phase = .done ∧ final.promotionCount = 1 ∧ final.pending = none ∧
      final.finishLease = none ∧
      ∃ tx, final.finishTx = some tx ∧ tx.candidate = candidate ∧
        tx.disposition = disposition ∧ tx.promotionReceipt = true ∧
        tx.cleanupCommitted = true := by
  simp [exactOwner] at howner
  simp [finishPlan, run, reduce, exactOwner, exactFinishOwner, hversion, hrunning,
    howner, hsettled, hpending, haccepted, htx]

/-- Same-session recovery advances only process epochs, preserving attempt leases. -/
theorem deterministic_same_session_continuation
    (hversion : s.version = wireVersion)
    (hrunning : s.phase = .running)
    (howner : exactOwner s caller = true)
    (hpending : s.pending = some .resumeSame)
    (htx : s.finishTx = none)
    (hlease : s.finishLease = none)
    (hwrapper : caller.wrapperEpoch < newWrapperEpoch)
    (hchild : caller.childEpoch < newChildEpoch) :
    let final := (reduce s (.resumeSame caller newWrapperEpoch newChildEpoch)).1
    ∃ cap, final.owner = some cap ∧ final.worktreeLease = some cap ∧
      final.sessionLease = some cap ∧ cap.taskId = caller.taskId ∧
      cap.attemptId = caller.attemptId ∧ cap.generation = caller.generation ∧
      cap.fence = caller.fence ∧
      cap.wrapperIdentityDigest = caller.wrapperIdentityDigest ∧
      cap.childIdentityDigest = caller.childIdentityDigest ∧
      cap.ownedChild = caller.ownedChild ∧ final.pending = none := by
  simp [reduce, hversion, hrunning, howner, hpending, htx, hlease,
    Nat.not_le.mpr hwrapper, Nat.not_le.mpr hchild]

/-- Expected ownership contention is explicitly breaker-neutral. -/
theorem expected_contention_breaker_neutral :
    (reduce s (.ownershipContention caller)).1.breakerCharges = s.breakerCharges := by
  simp [reduce, reject]
  repeat' split <;> simp_all

/-- A crash after committed cleanup is already converged and crash replay is inert. -/
theorem replay_after_cleanup_is_converged
    (hdone : s.phase = .done) :
    (reduce s .crash).1 = s ∧ recoveryRank (reduce s .crash).1 = recoveryRank s := by
  by_cases hversion : s.version = wireVersion <;>
    simp [reduce, reject, hversion, hdone]

/-- Strict rank descent cannot execute forever because the finish rank is a natural. -/
theorem rank_decreasing_recovery_is_well_founded :
    WellFounded (fun next current : State => recoveryRank next < recoveryRank current) := by
  exact (measure recoveryRank).wf

/-- Fair restart makes a pending deterministic convergence action schedulable. -/
theorem settled_or_dead_eventually_scheduled
    (env : EnvironmentAssumptions cap durable)
    (hr : Reachable cap s)
    (hpending : s.pending.isSome = true) :
    ∃ event, DeterministicRecoveryAction s event := by
  have hdurable := env.eventualRestart hr hpending
  exact env.fairScheduling hr hdurable hpending

/--
Conditional convergence boundary: under the explicit durable/restart/fair
scheduler assumptions, a settled/proven-dead attempt is not inert. Each
scheduled transition preserves safety; the pure finite plans above establish
its two possible targets: same-session/worktree continuation or exactly-once
disposition plus cleanup.
-/
theorem conditional_convergence
    (env : EnvironmentAssumptions cap durable)
    (hr : Reachable cap s)
    (hpending : s.pending.isSome = true)
    (hsafe : ProtocolInvariant s) :
    ∃ event, DeterministicRecoveryAction s event ∧
      ProtocolInvariant (reduce s event).1 := by
  obtain ⟨event, haction⟩ := settled_or_dead_eventually_scheduled env hr hpending
  exact ⟨event, haction, reduce_preserves_invariant hsafe⟩

end WGLifecycle

namespace WGLifecycle.V2

/-- Missing monotone commits from GraphSave; holds have no automatic rank. -/
def recoveryRank : SavePhase → Nat
  | .absent => 13 | .prepared => 12 | .quiescing => 11 | .workSaved => 10
  | .candidateSealed => 9 | .validated => 8 | .awaitingAcceptance => 7
  | .accepted => 6 | .dispositionRecorded => 5 | .effectPrepared => 4
  | .effectCommitted => 3 | .cleanupPrepared => 2 | .cleanupCommitted => 1
  | .graphSaved => 0
  | _ => 14

/-- Every normal durable edge strictly lowers recovery rank. -/
theorem durable_edge_decreases_rank
    (h : legalEdge phase next = true)
    (hnormal : next ≠ .needsRepair ∧ next ≠ .upgradeBlocked ∧
      next ≠ .needsReconciliation ∧ next ≠ .abortedPreserved) :
    recoveryRank next < recoveryRank phase := by
  cases phase <;> cases next <;> simp [legalEdge, recoveryRank] at h hnormal ⊢

/-- Retry is a new generation and requires an already saved prior attempt. -/
theorem reset_is_new_generation
    (hversion : s.version = wireVersion) (hsource : source = s.source)
    (hsaved : s.workSaved = true) :
    (reduce s (.retry source)).1.generation = source.generation + 1 := by
  subst source
  simp [reduce, initial, hversion, hsaved]

/-- Same-attempt continuation is inert unless the exact proof and tuple agree. -/
theorem resume_same_requires_exact_continuation_proof
    (hversion : s.version = wireVersion) :
    (reduce s (.resumeSame source false)).1 = s := by
  simp [reduce, hversion]

/-- Conditional liveness names adapter obligations without modeling them. -/
structure EnvironmentAssumptions where
  committedWritesSurviveOrFailDetectably : Prop
  collisionResistanceAndAtomicCas : Prop
  truthfulQuiescenceAndRootObservations : Prop
  completeExclusionPolicy : Prop
  eventualCompatibleFairReplay : Prop
  unsupportedAdaptersFailClosed : Prop

/-- A useful, verified normal edge is available when supplied by fair adapters. -/
theorem dead_owner_not_parked_conditionally
    (_env : EnvironmentAssumptions)
    (hversion : s.version = wireVersion) (hsource : source = s.source)
    (hedge : legalEdge s.phase next = true) (hverified : verified = true) :
    (reduce s (.advance source next verified)).2 = .applied := by
  subst source
  simp [reduce, hversion, hedge, hverified]

end WGLifecycle.V2
