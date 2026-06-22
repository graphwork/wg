// Standalone replica of the patched resolveAppMode + the upstream isTruthyEnvFlag
// helper, to prove the PI_NO_TUI guard's truth table.
function isTruthyEnvFlag(value) {
  if (!value) return false;
  return value === "1" || value.toLowerCase() === "true" || value.toLowerCase() === "yes";
}
function resolveAppMode(parsed, stdinIsTTY, stdoutIsTTY, env) {
  if (isTruthyEnvFlag(env.PI_NO_TUI) && parsed.mode === undefined && !parsed.print) {
    return "print";
  }
  if (parsed.mode === "rpc")  return "rpc";
  if (parsed.mode === "json") return "json";
  if (parsed.print || !stdinIsTTY || !stdoutIsTTY) return "print";
  return "interactive";
}

const cases = [
  // [label, parsed, stdinTTY, stdoutTTY, env, expected]
  ["default-off, both TTY, no flags -> interactive (UNCHANGED)", {}, true, true, {}, "interactive"],
  ["PI_NO_TUI unset, piped stdin -> print (UNCHANGED)",          {}, false, true, {}, "print"],
  ["PI_NO_TUI=1, both TTY, no flags -> print (GUARD FIRES)",     {}, true, true, {PI_NO_TUI:"1"}, "print"],
  ["PI_NO_TUI=true, both TTY -> print (GUARD FIRES)",            {}, true, true, {PI_NO_TUI:"true"}, "print"],
  ["PI_NO_TUI=yes, both TTY -> print (GUARD FIRES)",             {}, true, true, {PI_NO_TUI:"yes"}, "print"],
  ["PI_NO_TUI=0 -> interactive (falsey, default-off)",          {}, true, true, {PI_NO_TUI:"0"}, "interactive"],
  ["PI_NO_TUI=false -> interactive (falsey)",                   {}, true, true, {PI_NO_TUI:"false"}, "interactive"],
  ["PI_NO_TUI=1 + --mode rpc -> rpc (EXPLICIT WINS)",           {mode:"rpc"}, true, true, {PI_NO_TUI:"1"}, "rpc"],
  ["PI_NO_TUI=1 + --mode json -> json (EXPLICIT WINS)",         {mode:"json"}, true, true, {PI_NO_TUI:"1"}, "json"],
  ["PI_NO_TUI=1 + -p/--print -> print (print path)",           {print:true}, true, true, {PI_NO_TUI:"1"}, "print"],
  ["no env + --mode rpc -> rpc (UNCHANGED)",                    {mode:"rpc"}, true, true, {}, "rpc"],
];
let fail = 0;
for (const [label, parsed, si, so, env, exp] of cases) {
  const got = resolveAppMode(parsed, si, so, env);
  const ok = got === exp;
  if (!ok) fail++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${got.padEnd(11)} (exp ${exp.padEnd(11)})  ${label}`);
}
console.log(fail === 0 ? "\nALL PASS — default-off + composes-with-mode verified" : `\n${fail} FAILURES`);
process.exit(fail === 0 ? 0 : 1);
