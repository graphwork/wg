import WGLifecycle.SimpleLand
import Lean.Data.Json.FromToJson
import Lean.Data.Json.Parser

open Lean
open WGLifecycle.SimpleLand

structure OracleRequest where
  state : State
  events : List Event
  deriving ToJson, FromJson

structure OracleResponse where
  state : State
  decisions : List Decision
  deriving ToJson, FromJson

private def replayWithDecisions : State → List Event → State × List Decision
  | state, [] => (state, [])
  | state, event :: rest =>
      let result := reduce state event
      let tail := replayWithDecisions result.1 rest
      (tail.1, result.2 :: tail.2)

def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let input ← stdin.readToEnd
  match Json.parse input >>= (fromJson? : Json → Except String OracleRequest) with
  | .error error =>
      IO.eprintln s!"simple-land-oracle: {error}"
      return 2
  | .ok request =>
      let result := replayWithDecisions request.state request.events
      let response : OracleResponse := { state := result.1, decisions := result.2 }
      IO.println (toJson response).compress
      return 0
