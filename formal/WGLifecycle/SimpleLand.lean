import Std.Tactic

namespace WGLifecycle.SimpleLand

/-- Wire version for the deliberately small worker-owned completion kernel. -/
def wireVersion : Nat := 1

inductive Contract where
  | land | report | explore
  deriving DecidableEq, Repr

inductive Phase where
  | working | reviewBlocked | reviewUnavailable | accepted | published | done | failed
  deriving DecidableEq, Repr

inductive ReviewVerdict where
  | absent | pass | reject | unavailable | incompleteEvidence
  deriving DecidableEq, Repr

/-- Finite symbolic digests/OIDs. Rust adapters own byte-level verification. -/
structure Manifest where
  id : Nat
  requirements : Nat
  contract : Contract
  outputDigest : Nat
  validationDigest : Nat
  integratedMain : Nat
  allResolvable : Bool
  protectedFree : Bool
  deriving DecidableEq, Repr

structure ReviewReceipt where
  manifest : Nat
  requirements : Nat
  verdict : ReviewVerdict
  deriving DecidableEq, Repr

structure PublicationReceipt where
  manifest : Nat
  outputDigest : Nat
  deriving DecidableEq, Repr

structure State where
  phase : Phase
  manifest : Option Manifest
  flip : ReviewReceipt
  eval : ReviewReceipt
  publication : Option PublicationReceipt
  publicationCount : Nat
  failureCode : Option Nat
  deriving DecidableEq, Repr

inductive Decision where
  | applied | noop | rejected
  deriving DecidableEq, Repr

inductive Event where
  | submitManifest (manifest : Manifest)
  | recordFlip (manifest requirements : Nat) (verdict : ReviewVerdict)
  | recordEval (manifest requirements : Nat) (verdict : ReviewVerdict)
  /-- `observedMain` is used only by Land. `succeeded` is the verified adapter
  publication result for every contract. -/
  | publishObserved (manifest observedMain : Nat) (succeeded outputsMatch : Bool)
  | complete (manifest : Nat) (outputsStillResolve : Bool)
  | fail (code : Nat)
  | retry
  deriving DecidableEq, Repr

private def absentReceipt : ReviewReceipt :=
  { manifest := 0, requirements := 0, verdict := .absent }

private def passingReceipt (manifest : Manifest) : ReviewReceipt :=
  { manifest := manifest.id, requirements := manifest.requirements, verdict := .pass }

def initial : State := {
  phase := .working
  manifest := none
  flip := absentReceipt
  eval := absentReceipt
  publication := none
  publicationCount := 0
  failureCode := none
}

private def exactPass (receipt : ReviewReceipt) (manifest : Manifest) : Bool :=
  receipt.manifest = manifest.id &&
    receipt.requirements = manifest.requirements &&
    receipt.verdict = .pass

private def reviewPhase (verdict : ReviewVerdict) : Phase :=
  match verdict with
  | .reject | .incompleteEvidence => .reviewBlocked
  | .unavailable => .reviewUnavailable
  | _ => .working

private def targetMatches (manifest : Manifest) (observedMain : Nat) : Bool :=
  match manifest.contract with
  | .land => observedMain = manifest.integratedMain
  | .report | .explore => true

private def reject (s : State) : State × Decision := (s, .rejected)

/-- Pure universal review-valve and publication reducer. It contains no spawn,
retry timer, cleanup, route, process, archive, or replacement-worker action. -/
def reduce (s : State) (event : Event) : State × Decision :=
  if s.phase = .done then (s, .noop)
  else match event with
  | .submitManifest manifest =>
      if !manifest.allResolvable || !manifest.protectedFree ||
          manifest.id = 0 || manifest.requirements = 0 || manifest.outputDigest = 0 ||
          manifest.validationDigest = 0 then
        reject s
      else if s.manifest = some manifest then
        (s, .noop)
      else
        ({ s with
          phase := .working
          manifest := some manifest
          flip := absentReceipt
          eval := absentReceipt
          publication := none
          publicationCount := 0
          failureCode := none }, .applied)
  | .recordFlip manifest requirements verdict =>
      match s.manifest with
      | none => reject s
      | some candidate =>
          if candidate.id != manifest || candidate.requirements != requirements ||
              verdict = .absent then
            reject s
          else
            ({ s with
              phase := reviewPhase verdict
              flip := { manifest, requirements, verdict }
              eval := absentReceipt
              publication := none
              publicationCount := 0 }, .applied)
  | .recordEval manifest requirements verdict =>
      match s.manifest with
      | none => reject s
      | some candidate =>
          if candidate.id != manifest || candidate.requirements != requirements ||
              s.flip != passingReceipt candidate || verdict = .absent then
            reject s
          else
            let nextPhase := if verdict = .pass then Phase.accepted else reviewPhase verdict
            ({ s with
              phase := nextPhase
              eval := { manifest, requirements, verdict }
              publication := none
              publicationCount := 0 }, .applied)
  | .publishObserved manifest observedMain succeeded outputsMatch =>
      match s.manifest with
      | none => reject s
      | some candidate =>
          if s.phase != .accepted || candidate.id != manifest ||
              s.flip != passingReceipt candidate || s.eval != passingReceipt candidate ||
              !targetMatches candidate observedMain || !succeeded || !outputsMatch then
            reject s
          else
            let receipt := { manifest := candidate.id, outputDigest := candidate.outputDigest }
            ({ s with
              phase := .published
              publication := some receipt
              publicationCount := 1 }, .applied)
  | .complete manifest outputsStillResolve =>
      match s.manifest, s.publication with
      | some candidate, some publication =>
          if s.phase != .published || candidate.id != manifest ||
              publication.manifest != candidate.id ||
              publication.outputDigest != candidate.outputDigest ||
              s.flip != passingReceipt candidate || s.eval != passingReceipt candidate ||
              !outputsStillResolve then
            reject s
          else
            ({ s with phase := .done, failureCode := none }, .applied)
      | _, _ => reject s
  | .fail code =>
      if s.phase = .published then (s, .noop)
      else ({ s with phase := .failed, failureCode := some code }, .applied)
  | .retry =>
      if s.phase != .failed && s.phase != .reviewBlocked && s.phase != .reviewUnavailable then
        reject s
      else
        ({ s with
          phase := .working
          flip := absentReceipt
          eval := absentReceipt
          publication := none
          publicationCount := 0
          failureCode := none }, .applied)

def run : State → List Event → State
  | s, [] => s
  | s, event :: rest => run (reduce s event).1 rest

def Step (s : State) (event : Event) (s' : State) : Prop :=
  (reduce s event).1 = s'

inductive Reachable : State → Prop where
  | init : Reachable initial
  | step : Reachable s → Step s event s' → Reachable s'

/-- Done is exactly the universal valve plus contract publication projection. -/
def DoneInvariant (s : State) : Prop :=
  s.phase = .done →
    ∃ manifest publication,
      s.manifest = some manifest ∧
      s.flip = passingReceipt manifest ∧
      s.eval = passingReceipt manifest ∧
      s.publication = some publication ∧
      publication.manifest = manifest.id ∧
      publication.outputDigest = manifest.outputDigest ∧
      s.publicationCount = 1

/-- Every accepted publication is bound to exact passing reviews and output. -/
def PublicationInvariant (s : State) : Prop :=
  s.publication.isSome = true →
    ∃ manifest publication,
      s.manifest = some manifest ∧
      s.flip = passingReceipt manifest ∧
      s.eval = passingReceipt manifest ∧
      s.publication = some publication ∧
      publication.manifest = manifest.id ∧
      publication.outputDigest = manifest.outputDigest ∧
      s.publicationCount = 1

def ProtocolInvariant (s : State) : Prop :=
  s.publicationCount ≤ 1 ∧ DoneInvariant s ∧ PublicationInvariant s

private theorem initial_invariant : ProtocolInvariant initial := by
  simp [ProtocolInvariant, initial, DoneInvariant, PublicationInvariant]

set_option maxHeartbeats 2000000 in
theorem reduce_preserves_invariant
    (h : ProtocolInvariant s) : ProtocolInvariant (reduce s event).1 := by
  rcases h with ⟨hcount, hdone, hpublication⟩
  cases event <;>
    simp [reduce, reject, ProtocolInvariant, DoneInvariant,
      PublicationInvariant, targetMatches, reviewPhase, passingReceipt] at *
  all_goals repeat' split
  all_goals simp_all
  all_goals grind

theorem reachable_protocol_invariant
    (hr : Reachable s) : ProtocolInvariant s := by
  induction hr with
  | init => exact initial_invariant
  | step hr hstep ih =>
      rw [← hstep]
      exact reduce_preserves_invariant ih

/-- Every Done task passed both reviewers for the exact published manifest. -/
theorem done_implies_exact_universal_review
    (hr : Reachable s) (hdone : s.phase = .done) :
    ∃ manifest publication,
      s.manifest = some manifest ∧
      s.flip.manifest = manifest.id ∧ s.flip.requirements = manifest.requirements ∧
      s.flip.verdict = .pass ∧
      s.eval.manifest = manifest.id ∧ s.eval.requirements = manifest.requirements ∧
      s.eval.verdict = .pass ∧
      s.publication = some publication ∧
      publication.manifest = manifest.id ∧
      publication.outputDigest = manifest.outputDigest := by
  rcases reachable_protocol_invariant hr with ⟨_, hsafe, _⟩
  rcases hsafe hdone with ⟨manifest, publication, hm, hf, he, hp, hpm, hpo, _⟩
  subst_vars
  simp_all [passingReceipt]

/-- A changed manifest discards both old review receipts and publication state. -/
theorem changed_manifest_invalidates_review
    (hvalid : manifest.allResolvable = true ∧ manifest.protectedFree = true ∧
      manifest.id ≠ 0 ∧ manifest.requirements ≠ 0 ∧ manifest.outputDigest ≠ 0 ∧
      manifest.validationDigest ≠ 0)
    (hchanged : s.manifest ≠ some manifest)
    (hnondone : s.phase ≠ .done) :
    let next := (reduce s (.submitManifest manifest)).1
    next.flip.verdict = .absent ∧ next.eval.verdict = .absent ∧
      next.publication = none := by
  rcases hvalid with ⟨hresolve, hprotected, hid, hrequirements, houtput, hvalidation⟩
  simp [reduce, hnondone, hresolve, hprotected, hid, hrequirements,
    houtput, hvalidation, hchanged, absentReceipt]

/-- A moved main cannot publish a Land candidate. -/
theorem stale_main_cannot_land
    (hnondone : s.phase ≠ .done)
    (hmanifest : s.manifest = some manifest)
    (hland : manifest.contract = .land)
    (hstale : observedMain ≠ manifest.integratedMain) :
    (reduce s (.publishObserved manifest.id observedMain true true)).1 = s := by
  simp [reduce, hnondone, hmanifest, targetMatches, hland, hstale, reject]

/-- Rejected, unavailable, or incomplete review cannot directly publish. -/
theorem nonpassing_eval_cannot_publish
    (hnondone : s.phase ≠ .done)
    (hmanifest : s.manifest = some manifest)
    (hnonpass : s.eval.verdict ≠ .pass) :
    (reduce s (.publishObserved manifest.id manifest.integratedMain true true)).1 = s := by
  have hreceipt : s.eval ≠ passingReceipt manifest := by
    intro heq
    have hverdict := congrArg ReviewReceipt.verdict heq
    exact hnonpass (by simpa [passingReceipt] using hverdict)
  simp [reduce, hnondone, hmanifest, hreceipt, reject]

/-- Once Done, every later diagnostic/control event is inert. -/
theorem terminal_success_is_inert
    (hdone : s.phase = .done) : (reduce s event).1 = s := by
  simp [reduce, hdone]

/-- The event language contains no automatic source spawn or replacement action.
This exhaustive eliminator is the formal API surface used by Rust conformance. -/
def Event.isSourceReplacement : Event → Bool
  | .submitManifest _ | .recordFlip _ _ _ | .recordEval _ _ _
  | .publishObserved _ _ _ _ | .complete _ _ | .fail _ | .retry => false

theorem no_event_replaces_source (event : Event) :
    event.isSourceReplacement = false := by
  cases event <;> rfl

end WGLifecycle.SimpleLand
