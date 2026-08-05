namespace WGLifecycle

/-- Wire schema version shared with `src/lifecycle_protocol.rs`. -/
def wireVersion : Nat := 1

/-- Exact runtime reducer version in `service/convergence.rs`. -/
def exitedWorkerFinishReducerVersion : Nat := 1

inductive Phase where
  | running | done | failed
  deriving DecidableEq, Repr

inductive Disposition where
  | land | deliver | report
  deriving DecidableEq, Repr

inductive PendingAction where
  | resumeSame | beginFinish | promote | cleanup
  deriving DecidableEq, Repr

/-- Byte-name equivalents of the production exited-worker convergence wire. -/
inductive FinishConvergenceRank where
  | awaitReceipt | receiptNoTransaction | transactionDurable | promoted | cleaned
  deriving DecidableEq, Repr

inductive FinishConvergenceAction where
  | waitForReceipt | resumeSameSession | advanceTransaction | promote | cleanup
  | complete | rejectStale
  deriving DecidableEq, Repr

structure Capability where
  taskId : Nat
  generation : Nat
  attemptId : Nat
  fence : Nat
  wrapperEpoch : Nat
  childEpoch : Nat
  wrapperIdentityDigest : Nat
  childIdentityDigest : Nat
  ownedChild : Bool
  deriving DecidableEq, Repr

structure Candidate where
  id : Nat
  baseCas : Nat
  protectedFree : Bool
  deriving DecidableEq, Repr

structure FinishTx where
  candidate : Candidate
  disposition : Disposition
  promotionReceipt : Bool
  cleanupCommitted : Bool
  deriving DecidableEq, Repr

structure State where
  version : Nat
  phase : Phase
  owner : Option Capability
  worktreeLease : Option Capability
  sessionLease : Option Capability
  finishLease : Option Capability
  settled : Bool
  ownerProvenDead : Bool
  pending : Option PendingAction
  deadline : Option Nat
  candidate : Option Candidate
  accepted : Option Candidate
  finishTx : Option FinishTx
  promotionCount : Nat
  breakerCharges : Nat
  inertMessages : Nat
  deriving DecidableEq, Repr

inductive RejectReason where
  | wireVersion | staleCapability | invalidTopology | untruthfulDeathObservation
  | candidateNotProtected | candidateMismatch | candidateNotAccepted
  | missingFinishTx | casMoved | invalidPhase | invalidRecoveryAction
  deriving DecidableEq, Repr

inductive Decision where
  | applied | noop | rejected (reason : RejectReason)
  deriving DecidableEq, Repr

inductive Event where
  | candidateValidated (caller : Capability) (candidate : Candidate)
  | childSettled (caller : Capability) (candidate : Candidate) (deadline : Nat)
  | ownerProvenDead (caller : Capability) (truthful : Bool) (deadline : Nat)
  | wrapperHandoff (caller : Capability) (disposition : Disposition) (deadline : Nat)
  | resumeSame (caller : Capability) (newWrapperEpoch newChildEpoch : Nat)
  | beginFinish (caller : Capability) (disposition : Disposition)
  | promote (caller : Capability) (candidateId baseCas currentBaseCas : Nat)
  | commitCleanup (caller : Capability)
  | fail (caller : Capability)
  | ownershipContention (caller : Capability)
  | message (body : Nat)
  | crash
  deriving DecidableEq, Repr

def initial (cap : Capability) : State := {
  version := wireVersion
  phase := .running
  owner := some cap
  worktreeLease := some cap
  sessionLease := some cap
  finishLease := none
  settled := false
  ownerProvenDead := false
  pending := none
  deadline := none
  candidate := none
  accepted := none
  finishTx := none
  promotionCount := 0
  breakerCharges := 0
  inertMessages := 0
}

def exactOwner (s : State) (caller : Capability) : Bool :=
  decide (s.owner = some caller ∧
    s.worktreeLease = some caller ∧ s.sessionLease = some caller)

def exactFinishOwner (s : State) (caller : Capability) : Bool :=
  exactOwner s caller && decide (s.finishLease = some caller)

def reject (s : State) (reason : RejectReason) : State × Decision :=
  (s, .rejected reason)

/--
Executable reference transition function. Rejections and crash observations are
inert. The exact current wrapper capability authorizes handoff for its owned
native child; reversed OS ancestry is deliberately absent from the model.
-/
def reduce (s : State) (event : Event) : State × Decision :=
  if s.version != wireVersion then reject s .wireVersion
  else if s.phase != .running then (s, .noop)
  else match event with
  | .message _ => ({ s with inertMessages := s.inertMessages + 1 }, .applied)
  | .crash => (s, .noop)
  | .ownershipContention _ => (s, .noop)
  | .candidateValidated caller candidate =>
      if !exactOwner s caller then reject s .staleCapability
      else if !candidate.protectedFree then reject s .candidateNotProtected
      else match s.accepted with
      | some existing =>
          if existing = candidate then (s, .noop) else reject s .candidateMismatch
      | none =>
          ({ s with
            candidate := some candidate
            accepted := some candidate }, .applied)
  | .childSettled caller candidate deadline =>
      if !exactOwner s caller then reject s .staleCapability
      else if !candidate.protectedFree then reject s .candidateNotProtected
      else if s.accepted != some candidate then reject s .candidateNotAccepted
      else
        ({ s with
          settled := true
          candidate := some candidate
          pending := some .beginFinish
          deadline := some deadline }, .applied)
  | .ownerProvenDead caller truthful deadline =>
      if !exactOwner s caller then reject s .staleCapability
      else if !truthful then reject s .untruthfulDeathObservation
      else
        let pending := match s.finishTx with
          | some tx => if tx.promotionReceipt then PendingAction.cleanup else PendingAction.promote
          | none => PendingAction.resumeSame
        ({ s with
          ownerProvenDead := true
          pending := some pending
          deadline := some deadline }, .applied)
  | .wrapperHandoff caller disposition deadline =>
      if !exactOwner s caller then reject s .staleCapability
      else if s.ownerProvenDead || caller.wrapperEpoch = 0 || caller.childEpoch = 0 ||
        caller.wrapperIdentityDigest = 0 || caller.childIdentityDigest = 0 ||
        !caller.ownedChild then reject s .invalidTopology
      else match s.accepted with
      | none => reject s .candidateNotAccepted
      | some candidate => match s.finishTx with
        | some tx => if tx.candidate = candidate && tx.disposition = disposition
          then (s, .noop) else reject s .candidateMismatch
        | none =>
          let tx : FinishTx := {
            candidate := candidate
            disposition := disposition
            promotionReceipt := false
            cleanupCommitted := false }
          ({ s with
            settled := true
            finishTx := some tx
            finishLease := some caller
            pending := some .promote
            deadline := some deadline }, .applied)
  | .resumeSame caller newWrapperEpoch newChildEpoch =>
      if !exactOwner s caller then reject s .staleCapability
      else if s.pending != some .resumeSame || s.finishTx.isSome || s.finishLease.isSome ||
        newWrapperEpoch ≤ caller.wrapperEpoch || newChildEpoch ≤ caller.childEpoch then
        reject s .invalidRecoveryAction
      else
        let cap := { caller with
          wrapperEpoch := newWrapperEpoch
          childEpoch := newChildEpoch }
        ({ s with
          owner := some cap
          worktreeLease := some cap
          sessionLease := some cap
          finishLease := none
          settled := false
          ownerProvenDead := false
          pending := none
          deadline := none }, .applied)
  | .beginFinish caller disposition =>
      if !exactOwner s caller then reject s .staleCapability
      else if s.pending != some .beginFinish then reject s .invalidRecoveryAction
      else match s.accepted with
      | none => reject s .candidateNotAccepted
      | some candidate => match s.finishTx with
        | some _ => (s, .noop)
        | none =>
          let tx : FinishTx := {
            candidate := candidate
            disposition := disposition
            promotionReceipt := false
            cleanupCommitted := false }
          ({ s with
            finishTx := some tx
            finishLease := some caller
            pending := some .promote }, .applied)
  | .promote caller candidateId baseCas currentBaseCas =>
      if !exactFinishOwner s caller then reject s .staleCapability
      else match s.finishTx with
      | none => reject s .missingFinishTx
      | some tx =>
        if tx.promotionReceipt then (s, .noop)
        else if tx.candidate.id != candidateId || tx.candidate.baseCas != baseCas then
          reject s .candidateMismatch
        else if currentBaseCas != baseCas then reject s .casMoved
        else
          let tx' := { tx with promotionReceipt := true }
          ({ s with
            finishTx := some tx'
            promotionCount := 1
            pending := some .cleanup }, .applied)
  | .commitCleanup caller =>
      if !exactFinishOwner s caller then reject s .staleCapability
      else match s.finishTx with
      | none => reject s .missingFinishTx
      | some tx =>
        if !tx.promotionReceipt then reject s .invalidRecoveryAction
        else
          let tx' := { tx with cleanupCommitted := true }
          ({ s with
            finishTx := some tx'
            owner := none
            worktreeLease := none
            sessionLease := none
            finishLease := none
            pending := none
            deadline := none
            phase := .done }, .applied)
  | .fail caller =>
      if !exactOwner s caller then reject s .staleCapability
      else if s.accepted.isSome || s.finishTx.isSome then reject s .invalidPhase
      else
        ({ s with
          phase := .failed
          owner := none
          worktreeLease := none
          sessionLease := none
          pending := none
          deadline := none }, .applied)

/-- Execute a deterministic finite replay plan. -/
def run : State → List Event → State
  | s, [] => s
  | s, event :: rest => run (reduce s event).1 rest

/-- Transition relation mechanically induced by the executable reducer. -/
def Step (s : State) (event : Event) (s' : State) : Prop :=
  (reduce s event).1 = s'

inductive Reachable (cap : Capability) : State → Prop where
  | init : Reachable cap (initial cap)
  | step : Reachable cap s → Step s event s' → Reachable cap s'

/-- Successful dependencies are exactly fully committed successful tasks. -/
def dependencySatisfied (s : State) : Prop :=
  s.phase = .done ∧ ∃ tx, s.finishTx = some tx ∧
    tx.promotionReceipt = true ∧ tx.cleanupCommitted = true

/-- Recovery rank for crash-replay: receipt/no-tx -> tx -> promoted -> cleaned. -/
def recoveryRank (s : State) : Nat :=
  match s.finishTx with
  | some tx => if tx.cleanupCommitted then 0 else if tx.promotionReceipt then 1 else 2
  | none => if s.settled && s.accepted.isSome then 3 else 0

/-- Normalized projection of the production `FinishConvergenceRank`. -/
def finishConvergenceRank (s : State) : FinishConvergenceRank :=
  match s.finishTx with
  | none => if s.settled then .receiptNoTransaction else .awaitReceipt
  | some tx => if tx.cleanupCommitted then .cleaned
    else if tx.promotionReceipt then .promoted else .transactionDurable

/--
Pure production exited-worker decision projected onto the formal state. Exact
capability equality and `ownedChild` are the topology authorization; no OS
ancestry relation appears.
-/
def finishConvergenceAction (presented authoritative : Capability)
    (s : State) : FinishConvergenceAction :=
  if !presented.ownedChild || presented != authoritative then .rejectStale
  else if !s.ownerProvenDead then .waitForReceipt
  else match s.finishTx with
  | none => .resumeSameSession
  | some tx => if tx.cleanupCommitted then .complete
    else if tx.promotionReceipt then .cleanup else .promote

end WGLifecycle

/-!
Version 2 is a new abstraction layered beside the completed version-1 program
above.  Receipt booleans denote facts already verified by Rust adapters; they
are not models of Git, storage, processes, or filesystems.
-/
namespace WGLifecycle.V2

def wireVersion : Nat := 2

inductive SavePhase where
  | absent | prepared | quiescing | workSaved | candidateSealed | validated
  | awaitingAcceptance | accepted | dispositionRecorded | effectPrepared
  | effectCommitted | cleanupPrepared | cleanupCommitted | graphSaved
  | needsRepair | abortedPreserved | upgradeBlocked | needsReconciliation
  deriving DecidableEq, Repr

structure Binding where
  generation : Nat
  attempt : Nat
  fence : Nat
  worktreeLease : Nat
  candidate : Nat
  base : Nat
  deriving DecidableEq, Repr

structure State where
  version : Nat
  source : Binding
  phase : SavePhase
  workSaved : Bool
  accepted : Bool
  effectCount : Nat
  cleanupCommitted : Bool
  graphSaveValid : Bool
  dependencySatisfied : Bool
  generation : Nat
  deriving DecidableEq, Repr

inductive Event where
  | advance (source : Binding) (next : SavePhase) (verified : Bool)
  | graphSave (source : Binding) (completeAgreeingBundle : Bool)
  | retry (source : Binding)
  | resumeSame (source : Binding) (exactContinuationProof : Bool)
  | legacyDone
  | incompatibleWire
  deriving DecidableEq, Repr

inductive Decision where
  | applied | noop | rejected
  deriving DecidableEq, Repr

def initial (source : Binding) : State := {
  version := wireVersion, source := source, phase := .absent,
  workSaved := false, accepted := false, effectCount := 0,
  cleanupCommitted := false, graphSaveValid := false,
  dependencySatisfied := false, generation := source.generation }

def legalEdge : SavePhase → SavePhase → Bool
  | .absent, .prepared | .prepared, .quiescing | .quiescing, .workSaved
  | .workSaved, .candidateSealed | .candidateSealed, .validated
  | .validated, .awaitingAcceptance | .validated, .accepted
  | .awaitingAcceptance, .accepted | .accepted, .dispositionRecorded
  | .dispositionRecorded, .effectPrepared | .effectPrepared, .effectCommitted
  | .effectCommitted, .cleanupPrepared | .cleanupPrepared, .cleanupCommitted => true
  | _, .needsRepair | _, .upgradeBlocked | _, .needsReconciliation => true
  | .prepared, .abortedPreserved | .quiescing, .abortedPreserved
  | .workSaved, .abortedPreserved => true
  | _, _ => false

def reduce (s : State) (event : Event) : State × Decision :=
  if s.version != wireVersion then (s, .rejected) else
  match event with
  | .incompatibleWire => ({ s with phase := .upgradeBlocked }, .applied)
  | .legacyDone => ({ s with
      phase := .needsReconciliation
      graphSaveValid := false
      dependencySatisfied := false }, .applied)
  | .resumeSame source proof =>
      if source != s.source || !proof then (s, .rejected) else (s, .noop)
  | .retry source =>
      if source != s.source || !s.workSaved then (s, .rejected)
      else ({ initial { source with generation := source.generation + 1 } with
        generation := source.generation + 1 }, .applied)
  | .graphSave source complete =>
      if source != s.source || s.phase != .cleanupCommitted || !complete then
        (s, .rejected)
      else ({ s with
        phase := .graphSaved
        graphSaveValid := true
        dependencySatisfied := true }, .applied)
  | .advance source next verified =>
      if source != s.source || !verified then (s, .rejected)
      else if !legalEdge s.phase next then
        if next = s.phase then (s, .noop) else (s, .rejected)
      else
        let effectCount := if next = .effectCommitted then s.effectCount + 1 else s.effectCount
        ({ s with
          phase := next
          workSaved := s.workSaved || next = .workSaved
          accepted := s.accepted || next = .accepted
          effectCount := effectCount
          cleanupCommitted := s.cleanupCommitted || next = .cleanupCommitted }, .applied)

def run : State → List Event → State
  | s, [] => s
  | s, event :: rest => run (reduce s event).1 rest

def completeAgreeingGraphSave (s : State) : Prop :=
  s.graphSaveValid = true ∧ s.workSaved = true ∧ s.accepted = true ∧
    s.effectCount = 1 ∧ s.cleanupCommitted = true

end WGLifecycle.V2
